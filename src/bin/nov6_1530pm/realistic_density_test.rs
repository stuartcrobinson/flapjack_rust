use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, ReloadPolicy};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::thread;
use rand::Rng;

#[cfg(target_os = "linux")]
fn get_rss_mb() -> Option<f64> {
    let stat = fs::read_to_string("/proc/self/status").ok()?;
    for line in stat.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1].parse::<u64>().ok().map(|kb| kb as f64 / 1024.0);
            }
        }
    }
    None
}

fn main() {
    println!("=== REALISTIC MULTI-TENANT DENSITY TEST ===\n");
    println!("Simulating production load pattern:");
    println!("  - 150 tenants × 50K docs each (disk-backed)");
    println!("  - Zipf query distribution (80/20 rule)");
    println!("  - Measure steady-state RSS after warmup\n");

    let base_dir = PathBuf::from("/tmp/flapjack_density_test");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir).unwrap();

    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT);
    let body = schema_builder.add_text_field("body", TEXT);
    let price = schema_builder.add_u64_field("price", FAST | STORED);
    let timestamp = schema_builder.add_u64_field("timestamp", FAST);
    let rating = schema_builder.add_u64_field("rating", FAST);
    let schema = schema_builder.build();

    let baseline_mb = get_rss_mb().unwrap();
    println!("Baseline RSS: {:.2} MB\n", baseline_mb);

    // Create 150 tenants
    let num_tenants = 150;
    let docs_per_tenant = 50_000;
    
    println!("Creating {} tenants with {}K docs each...", num_tenants, docs_per_tenant / 1000);
    let start = Instant::now();
    
    let mut readers = Vec::new();
    for tenant_id in 0..num_tenants {
        let tenant_dir = base_dir.join(format!("tenant_{}", tenant_id));
        fs::create_dir(&tenant_dir).unwrap();
        
        let index = Index::create_in_dir(&tenant_dir, schema.clone()).unwrap();
        let mut writer = index.writer(50_000_000).unwrap();
        
        for doc_id in 0..docs_per_tenant {
            writer.add_document(doc!(
                title => format!("Product {} for tenant {}", doc_id, tenant_id),
                body => "Generic product description with some searchable text content",
                price => (100 + (doc_id % 900)) as u64,
                timestamp => 1700000000 + doc_id as u64,
                rating => (1 + (doc_id % 5)) as u64,
            )).unwrap();
        }
        writer.commit().unwrap();
        drop(writer);
        
        let reader = index.reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into().unwrap();
        readers.push((index, reader));
        
        if (tenant_id + 1) % 30 == 0 {
            let elapsed = start.elapsed().as_secs();
            let current_mb = get_rss_mb().unwrap();
            println!("  Created {} tenants in {}s, RSS: {:.2} MB (+{:.2} MB)", 
                tenant_id + 1, elapsed, current_mb, current_mb - baseline_mb);
        }
    }
    
    let after_create_mb = get_rss_mb().unwrap();
    println!("\nAll tenants created.");
    println!("RSS: {:.2} MB (+{:.2} MB from baseline)", after_create_mb, after_create_mb - baseline_mb);
    println!("Per-tenant overhead: {:.2} MB\n", (after_create_mb - baseline_mb) / num_tenants as f64);

    // Warmup: Query hot tenants (top 20% by Zipf)
    println!("Warming up hot tenants (top 30 tenants, 80% of queries)...");
    let hot_tenants = 30;
    let mut rng = rand::thread_rng();
    
    for _ in 0..1000 {
        let tenant_idx = rng.gen_range(0..hot_tenants);
        let (_index, reader) = &readers[tenant_idx];
        let searcher = reader.searcher();
        let query_parser = QueryParser::for_index(&_index, vec![title, body]);
        let query = query_parser.parse_query("product").unwrap();
        let _ = searcher.search(&query, &TopDocs::with_limit(20)).unwrap();
    }
    
    thread::sleep(Duration::from_secs(2));
    let after_warmup_mb = get_rss_mb().unwrap();
    println!("After warmup RSS: {:.2} MB (+{:.2} MB)", after_warmup_mb, after_warmup_mb - baseline_mb);

    // Sustained query load (5000 queries over 30 seconds)
    println!("\nRunning sustained query load (Zipf distribution)...");
    let query_start = Instant::now();
    let mut query_count = 0;
    
    while query_start.elapsed() < Duration::from_secs(30) {
        // Zipf: 80% of queries hit top 20% of tenants
        let tenant_idx = if rng.gen::<f32>() < 0.8 {
            rng.gen_range(0..hot_tenants)
        } else {
            rng.gen_range(hot_tenants..num_tenants)
        };
        
        let (_index, reader) = &readers[tenant_idx];
        let searcher = reader.searcher();
        let query_parser = QueryParser::for_index(&_index, vec![title, body]);
        let query = query_parser.parse_query("product").unwrap();
        let _ = searcher.search(&query, &TopDocs::with_limit(20)).unwrap();
        query_count += 1;
        
        if query_count % 1000 == 0 {
            let current_mb = get_rss_mb().unwrap();
            println!("  {} queries, RSS: {:.2} MB", query_count, current_mb);
        }
    }
    
    let final_mb = get_rss_mb().unwrap();
    println!("\n=== FINAL RESULTS ===");
    println!("Queries executed: {}", query_count);
    println!("Steady-state RSS: {:.2} MB", final_mb);
    println!("Total overhead: {:.2} MB", final_mb - baseline_mb);
    println!("Per-tenant average: {:.2} MB", (final_mb - baseline_mb) / num_tenants as f64);
    println!("Hot tenant working set: {:.2} MB", (after_warmup_mb - after_create_mb));
    
    println!("\n=== DENSITY VERDICT ===");
    let per_tenant_mb = (final_mb - baseline_mb) / num_tenants as f64;
    let node_capacity_4gb = 4096.0 / per_tenant_mb;
    let node_capacity_16gb = 16384.0 / per_tenant_mb;
    
    println!("Per-tenant RSS: {:.2} MB", per_tenant_mb);
    println!("4 GB node capacity: {:.0} tenants", node_capacity_4gb);
    println!("16 GB node capacity: {:.0} tenants", node_capacity_16gb);
    
    if node_capacity_4gb >= 400.0 {
        println!("✓ Original 400 tenant/node target VIABLE on 4GB");
    } else if node_capacity_4gb >= 200.0 {
        println!("⚠ Reduced to ~{:.0} tenants/node on 4GB (2x cost vs plan)", node_capacity_4gb);
    } else if node_capacity_16gb >= 400.0 {
        println!("⚠ Need 16GB nodes for 400 tenant density (higher cost)");
    } else {
        println!("✗ Architecture revision needed - memory unbounded");
    }
    
    // Cleanup
    let _ = fs::remove_dir_all(&base_dir);
}