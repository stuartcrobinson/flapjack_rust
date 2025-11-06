// Test: Can Tantivy achieve 400 tenants/4GB with sequential batched writes?
//
// Context: write_latency_test.rs showed 23.4 MB/tenant under concurrent commits.
// Hypothesis: Overhead was from fsync serialization, not memory. Sequential 
// batching might achieve 8.5 MB/tenant (single-threaded baseline).
//
// Critical question: If yes, LMDB's density advantage disappears.

use tantivy::{Index, IndexWriter, doc, schema::*};
use tempfile::TempDir;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TENANT_COUNT: usize = 400;
const DOCS_PER_TENANT: usize = 1000; // Small corpus for faster test
const BATCH_SIZE: usize = 100;

fn get_rss_mb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status")
        .or_else(|_| std::fs::read_to_string(format!("/proc/{}/status", std::process::id())))
        .unwrap_or_default();
    
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let kb: u64 = line.split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb as f64 / 1024.0;
        }
    }
    0.0
}

struct TenantIndex {
    index: Index,
    writer: Arc<Mutex<IndexWriter>>,
    temp_dir: TempDir,
}

fn create_tenant_index() -> TenantIndex {
    let temp_dir = TempDir::new().unwrap();
    
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    let schema = schema_builder.build();
    
    let index = Index::create_in_dir(&temp_dir, schema.clone()).unwrap();
    let writer = index.writer(50_000_000).unwrap(); // 50MB buffer
    
    TenantIndex {
        index,
        writer: Arc::new(Mutex::new(writer)),
        temp_dir,
    }
}

fn main() {
    println!("\n=== Tantivy Sequential Batch Density Test ===\n");
    
    // Phase 1: Baseline
    let baseline_rss = get_rss_mb();
    println!("Phase 1: Baseline RSS = {:.2} MB", baseline_rss);
    
    // Phase 2: Create 400 tenant indices
    println!("\nPhase 2: Creating {} tenant indices...", TENANT_COUNT);
    let start = Instant::now();
    
    let tenants: Vec<TenantIndex> = (0..TENANT_COUNT)
        .map(|_| create_tenant_index())
        .collect();
    
    let after_create = get_rss_mb();
    let create_overhead = after_create - baseline_rss;
    println!("  Created in {:?}", start.elapsed());
    println!("  RSS = {:.2} MB", after_create);
    println!("  Overhead per tenant = {:.3} MB", create_overhead / TENANT_COUNT as f64);
    
    // Phase 3: Index documents with SEQUENTIAL batched writes
    println!("\nPhase 3: Indexing {} docs/tenant (sequential batches)...", DOCS_PER_TENANT);
    
    let write_queue: Arc<Mutex<Vec<(usize, Vec<String>)>>> = Arc::new(Mutex::new(Vec::new()));
    
    // Simulate multiple tenants queueing writes
    let queue_clone = write_queue.clone();
    thread::spawn(move || {
        for tenant_id in 0..TENANT_COUNT {
            let mut batch = Vec::new();
            for i in 0..DOCS_PER_TENANT {
                let doc = format!("tenant_{}_doc_{}_content", tenant_id, i);
                batch.push(doc);
                
                if batch.len() >= BATCH_SIZE {
                    queue_clone.lock().unwrap().push((tenant_id, batch.clone()));
                    batch.clear();
                }
            }
            if !batch.is_empty() {
                queue_clone.lock().unwrap().push((tenant_id, batch));
            }
        }
    });
    
    // Background thread: Sequential commit processing
    let commit_start = Instant::now();
    let mut commits_processed = 0;
    let mut total_commit_time = Duration::ZERO;
    
    loop {
        let work = {
            let mut queue = write_queue.lock().unwrap();
            if queue.is_empty() {
                if commits_processed >= TENANT_COUNT * (DOCS_PER_TENANT / BATCH_SIZE) {
                    break;
                }
                drop(queue);
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            queue.remove(0)
        };
        
        let (tenant_id, docs) = work;
        let tenant = &tenants[tenant_id];
        
        // Write batch
        let commit_t0 = Instant::now();
        {
            let mut writer = tenant.writer.lock().unwrap();
            let schema = tenant.index.schema();
            let title_field = schema.get_field("title").unwrap();
            let body_field = schema.get_field("body").unwrap();
            
            for doc_content in docs {
                let _ = writer.add_document(doc!(
                    title_field => doc_content.clone(),
                    body_field => doc_content
                ));
            }
            writer.commit().unwrap();
        }
        let commit_elapsed = commit_t0.elapsed();
        total_commit_time += commit_elapsed;
        commits_processed += 1;
        
        if commits_processed % 100 == 0 {
            println!("  Processed {} commits ({:.2}ms avg)", 
                commits_processed, 
                total_commit_time.as_secs_f64() * 1000.0 / commits_processed as f64
            );
        }
    }
    
    let after_index = get_rss_mb();
    let index_overhead = after_index - after_create;
    
    println!("\n  Indexed in {:?}", commit_start.elapsed());
    println!("  Total commits: {}", commits_processed);
    println!("  Avg commit latency: {:.2}ms", total_commit_time.as_secs_f64() * 1000.0 / commits_processed as f64);
    println!("  RSS = {:.2} MB", after_index);
    println!("  Index data overhead per tenant = {:.3} MB", index_overhead / TENANT_COUNT as f64);
    
    // Phase 4: Query all tenants (force pages resident)
    println!("\nPhase 4: Warming up all indices with queries...");
    
    for (i, tenant) in tenants.iter().enumerate() {
        let reader = tenant.index.reader().unwrap();
        let searcher = reader.searcher();
        let _ = searcher.search(&tantivy::query::AllQuery, &tantivy::collector::Count);
        
        if (i + 1) % 100 == 0 {
            println!("  Queried {}/{}", i + 1, TENANT_COUNT);
        }
    }
    
    let after_query = get_rss_mb();
    let query_overhead = after_query - after_index;
    
    println!("  RSS = {:.2} MB", after_query);
    println!("  Working set per tenant = {:.3} MB", query_overhead / TENANT_COUNT as f64);
    
    // Final Summary
    let total_overhead = after_query - baseline_rss;
    let per_tenant = total_overhead / TENANT_COUNT as f64;
    
    println!("\n=== FINAL RESULTS ===");
    println!("Tenants: {}", TENANT_COUNT);
    println!("Total RSS: {:.2} MB", after_query);
    println!("Total overhead: {:.2} MB", total_overhead);
    println!("Per-tenant overhead: {:.3} MB", per_tenant);
    println!("\nBreakdown:");
    println!("  Empty index: {:.3} MB", create_overhead / TENANT_COUNT as f64);
    println!("  + Index data: {:.3} MB", index_overhead / TENANT_COUNT as f64);
    println!("  + Working set: {:.3} MB", query_overhead / TENANT_COUNT as f64);
    println!("  = Total: {:.3} MB/tenant", per_tenant);
    
    // Decision matrix
    println!("\n=== DECISION MATRIX ===");
    if per_tenant < 10.0 {
        println!("✅ EXCELLENT: Tantivy achieves LMDB-equivalent density");
        println!("   Recommendation: Use Tantivy-only, skip LMDB");
        println!("   Enables segment replication for multi-region");
    } else if per_tenant < 15.0 {
        println!("⚠️  MARGINAL: Tantivy has 2-3x overhead vs LMDB (2.3 MB)");
        println!("   Density: {} tenants/4GB", (4096.0 / per_tenant) as usize);
        println!("   Recommendation: Test hybrid (LMDB single-region, Tantivy multi-region)");
    } else {
        println!("❌ FAILED: Tantivy overhead too high");
        println!("   Density: {} tenants/4GB", (4096.0 / per_tenant) as usize);
        println!("   Recommendation: LMDB required for density target");
    }
    
    println!("\nComparison to concurrent test:");
    println!("  Previous test (concurrent): 23.4 MB/tenant");
    println!("  This test (sequential): {:.1} MB/tenant", per_tenant);
    if per_tenant < 12.0 {
        println!("  ✅ Sequential batching ELIMINATES concurrent overhead");
    } else {
        println!("  ❌ Overhead remains high even with sequential commits");
    }
}