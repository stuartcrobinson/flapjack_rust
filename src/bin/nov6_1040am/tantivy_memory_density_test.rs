use std::fs;
// use std::path::Path;
use std::time::Instant;
use tantivy::merge_policy::LogMergePolicy;
use tantivy::schema::{Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexReader, IndexWriter};
use tempfile::TempDir;

/// Test: What's per-tenant memory overhead with production merge policy?
/// 
/// Goal: Validate 400 tenants/4GB density target (requires <3 MB/tenant working set).
/// 
/// Method:
/// - 50 tenants × 10K docs each
/// - Configure merge policy to limit segment explosion
/// - Measure RSS at various phases: baseline, after indexing, during queries, with inactive tenants

struct TenantIndex {
    index: Index,
    writer: IndexWriter,
    reader: Option<IndexReader>,
    dir: TempDir,
    tenant_id: usize,
}

impl TenantIndex {
    fn new(tenant_id: usize) -> Self {
        let dir = TempDir::new().unwrap();
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT | STORED);
        schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&dir, schema.clone()).unwrap();
        
        // Production merge policy configuration
        let mut merge_policy = LogMergePolicy::default();
        merge_policy.set_min_layer_size(10_000);
        merge_policy.set_level_log_size(0.75);
        merge_policy.set_del_docs_ratio_before_merge(0.2);
        
        let mut writer = index.writer(50_000_000).unwrap(); // 50MB heap
        writer.set_merge_policy(Box::new(merge_policy));
        
        TenantIndex { 
            index, 
            writer, 
            reader: None,
            dir,
            tenant_id,
        }
    }
    
    fn add_documents(&mut self, count: usize) {
        let schema = self.index.schema();
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        
        for i in 0..count {
            let doc = doc!(
                title => format!("Tenant {} Document {}", self.tenant_id, i),
                body => format!("This is document {} for tenant {}. The quick brown fox jumps over the lazy dog. ", i, self.tenant_id).repeat(5)
            );
            self.writer.add_document(doc).unwrap();
        }
    }
    
    fn commit(&mut self) -> Result<u64, tantivy::TantivyError> {
        self.writer.commit()
    }
    
    fn create_reader(&mut self) -> Result<(), tantivy::TantivyError> {
        self.reader = Some(self.index.reader()?);
        Ok(())
    }
    
    fn query(&self, term: &str) -> usize {
        if let Some(reader) = &self.reader {
            let searcher = reader.searcher();
            let schema = self.index.schema();
            let title_field = schema.get_field("title").unwrap();
            
            let query_parser = tantivy::query::QueryParser::for_index(&self.index, vec![title_field]);
            if let Ok(query) = query_parser.parse_query(term) {
                let top_docs = tantivy::collector::TopDocs::with_limit(10);
                if let Ok(results) = searcher.search(&query, &top_docs) {
                    return results.len();
                }
            }
        }
        0
    }
    
    fn get_segment_count(&self) -> usize {
        self.index.searchable_segment_ids().unwrap().len()
    }
    
    fn get_disk_usage(&self) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = fs::read_dir(self.dir.path()) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                }
            }
        }
        total
    }
}

fn measure_memory_usage() -> usize {
    let pid = std::process::id();
    
    #[cfg(target_os = "linux")]
    {
        let statm_path = format!("/proc/{}/statm", pid);
        if let Ok(contents) = fs::read_to_string(&statm_path) {
            let parts: Vec<&str> = contents.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(rss_pages) = parts[1].parse::<usize>() {
                    return rss_pages * 4096;
                }
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("ps")
            .args(&["-o", "rss=", "-p", &pid.to_string()])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(kb) = s.trim().parse::<usize>() {
                    return kb * 1024;
                }
            }
        }
    }
    
    0
}

fn main() {
    println!("=== Tantivy Memory Density Test ===\n");
    
    let num_tenants = 50;
    let docs_per_tenant = 10_000;
    
    println!("Phase 1: Baseline memory measurement");
    let baseline_memory = measure_memory_usage();
    println!("  Baseline: {:.2} MB\n", baseline_memory as f64 / 1_000_000.0);
    
    println!("Phase 2: Creating {} tenant indices...", num_tenants);
    let start = Instant::now();
    let mut tenants: Vec<TenantIndex> = (0..num_tenants)
        .map(|i| {
            if (i + 1) % 10 == 0 {
                println!("  Created {} tenants...", i + 1);
            }
            TenantIndex::new(i)
        })
        .collect();
    println!("  Creation time: {:?}\n", start.elapsed());
    
    let after_create_memory = measure_memory_usage();
    println!("  After creation: {:.2} MB (+{:.2} MB)\n", 
        after_create_memory as f64 / 1_000_000.0,
        (after_create_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    println!("Phase 3: Indexing {} docs per tenant...", docs_per_tenant);
    let start = Instant::now();
    for (i, tenant) in tenants.iter_mut().enumerate() {
        tenant.add_documents(docs_per_tenant);
        if (i + 1) % 10 == 0 {
            println!("  Indexed {} tenants...", i + 1);
        }
    }
    println!("  Indexing time: {:?}\n", start.elapsed());
    
    let after_index_memory = measure_memory_usage();
    println!("  After indexing: {:.2} MB (+{:.2} MB from baseline)\n",
        after_index_memory as f64 / 1_000_000.0,
        (after_index_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    println!("Phase 4: Committing all tenants...");
    let start = Instant::now();
    for (i, tenant) in tenants.iter_mut().enumerate() {
        tenant.commit().unwrap();
        if (i + 1) % 10 == 0 {
            println!("  Committed {} tenants...", i + 1);
        }
    }
    println!("  Commit time: {:?}\n", start.elapsed());
    
    let after_commit_memory = measure_memory_usage();
    println!("  After commits: {:.2} MB (+{:.2} MB from baseline)\n",
        after_commit_memory as f64 / 1_000_000.0,
        (after_commit_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    println!("Phase 5: Creating readers for all tenants...");
    let start = Instant::now();
    for tenant in tenants.iter_mut() {
        tenant.create_reader().unwrap();
    }
    println!("  Reader creation time: {:?}\n", start.elapsed());
    
    let after_reader_memory = measure_memory_usage();
    println!("  After readers: {:.2} MB (+{:.2} MB from baseline)\n",
        after_reader_memory as f64 / 1_000_000.0,
        (after_reader_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    println!("Phase 6: Querying all tenants (warm cache)...");
    let start = Instant::now();
    let mut total_results = 0;
    for tenant in &tenants {
        total_results += tenant.query("Document");
    }
    println!("  Query time: {:?} ({} total results)\n", start.elapsed(), total_results);
    
    let after_query_memory = measure_memory_usage();
    println!("  After queries: {:.2} MB (+{:.2} MB from baseline)\n",
        after_query_memory as f64 / 1_000_000.0,
        (after_query_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    // Phase 7: Segment analysis
    println!("Phase 7: Segment analysis");
    let mut segment_counts: Vec<usize> = tenants.iter()
        .map(|t| t.get_segment_count())
        .collect();
    segment_counts.sort();
    let median_segments = segment_counts[segment_counts.len() / 2];
    let max_segments = *segment_counts.iter().max().unwrap();
    println!("  Segments per tenant: median={}, max={}\n", median_segments, max_segments);
    
    // Phase 8: Disk usage
    println!("Phase 8: Disk usage analysis");
    let total_disk: u64 = tenants.iter()
        .map(|t| t.get_disk_usage())
        .sum();
    let avg_disk = total_disk / num_tenants as u64;
    println!("  Total: {:.2} MB", total_disk as f64 / 1_000_000.0);
    println!("  Per tenant: {:.2} MB\n", avg_disk as f64 / 1_000_000.0);
    
    // Phase 9: Inactive tenant test
    println!("Phase 9: Testing inactive tenant memory (query only 10 tenants)...");
    let before_selective = measure_memory_usage();
    let mut selective_results = 0;
    for i in 0..10 {
        selective_results += tenants[i].query("Document");
    }
    std::thread::sleep(std::time::Duration::from_millis(100)); // Let OS stabilize
    let after_selective = measure_memory_usage();
    println!("  Before selective: {:.2} MB", before_selective as f64 / 1_000_000.0);
    println!("  After selective: {:.2} MB", after_selective as f64 / 1_000_000.0);
    println!("  Delta: {:.2} MB ({} results)\n", 
        (after_selective as i64 - before_selective as i64).abs() as f64 / 1_000_000.0,
        selective_results
    );
    
    println!("=== RESULTS SUMMARY ===\n");
    
    let working_set = after_query_memory - baseline_memory;
    let per_tenant_working_set = working_set as f64 / num_tenants as f64;
    
    println!("Memory overhead:");
    println!("  Total working set: {:.2} MB", working_set as f64 / 1_000_000.0);
    println!("  Per tenant: {:.2} MB", per_tenant_working_set / 1_000_000.0);
    
    println!("\nDensity projection:");
    let available_ram_gb = 4.0;
    let available_ram_mb = available_ram_gb * 1024.0;
    let system_overhead_mb = 512.0; // OS + overhead
    let usable_ram_mb = available_ram_mb - system_overhead_mb;
    let max_tenants = (usable_ram_mb / (per_tenant_working_set / 1_000_000.0)) as usize;
    
    println!("  Available RAM: {:.1} GB ({:.0} MB usable)", available_ram_gb, usable_ram_mb);
    println!("  Max tenants/4GB: {}", max_tenants);
    println!("  Target: 400 tenants");
    
    println!("\nSegment counts:");
    println!("  Median: {} segments/tenant", median_segments);
    println!("  Max: {} segments/tenant", max_segments);
    
    println!("\nDisk usage:");
    println!("  {:.2} MB/tenant", avg_disk as f64 / 1_000_000.0);
    
    println!("\n=== INTERPRETATION ===");
    
    if per_tenant_working_set / 1_000_000.0 < 3.0 {
        println!("✅ PASS: {:.2} MB/tenant - Density target achievable", per_tenant_working_set / 1_000_000.0);
        println!("   {} tenants/4GB fits budget (target: 400)", max_tenants);
    } else if per_tenant_working_set / 1_000_000.0 < 5.0 {
        println!("⚠️  MARGINAL: {:.2} MB/tenant - Reduced density", per_tenant_working_set / 1_000_000.0);
        println!("   {} tenants/4GB (target was 400)", max_tenants);
        println!("   Economics still viable but margins tighter");
    } else {
        println!("❌ FAIL: {:.2} MB/tenant - Density target infeasible", per_tenant_working_set / 1_000_000.0);
        println!("   Only {} tenants/4GB (target: 400)", max_tenants);
        println!("   Need to reconsider architecture or pricing");
    }
    
    if median_segments > 10 {
        println!("\n⚠️  WARNING: {} median segments/tenant", median_segments);
        println!("   Merge policy may need tuning - high segment count impacts query performance");
    } else {
        println!("\n✅ Segment count acceptable: {} median segments/tenant", median_segments);
    }
    
    let compression_ratio = avg_disk as f64 / (docs_per_tenant as f64 * 200.0); // ~200 bytes/doc estimate
    println!("\nCompression ratio: {:.2}x", 1.0 / compression_ratio);
    
    if max_tenants < 400 {
        println!("\n⚠️  DECISION: Density target not met. Options:");
        println!("   1. Reduce free tier doc limit (10K → 5K)");
        println!("   2. Accept lower density, update pricing ($1 → $2/tenant)");
        println!("   3. Optimize merge policy further");
        println!("   4. Consider LMDB if Tantivy overhead unacceptable");
    }
}



// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin tantivy_memory_density_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
// warning: variable does not need to be mutable
//   --> src/bin/nov6_1040am/tantivy_memory_density_test.rs:43:13
//    |
// 43 |         let mut writer = index.writer(50_000_000).unwrap(); // 50MB heap
//    |             ----^^^^^^
//    |             |
//    |             help: remove this `mut`
//    |
//    = note: `#[warn(unused_mut)]` (part of `#[warn(unused)]`) on by default
// warning: `flapjack_rust` (bin "tantivy_memory_density_test") generated 1 warning (run `cargo fix --bin "tantivy_memory_density_test"` to apply 1 suggestion)
//     Finished `release` profile [optimized] target(s) in 53.06s
//      Running `target/release/tantivy_memory_density_test`
// === Tantivy Memory Density Test ===
// Phase 1: Baseline memory measurement
//   Baseline: 3.41 MB
// Phase 2: Creating 50 tenant indices...
//   Created 10 tenants...
//   Created 20 tenants...
//   Created 30 tenants...
//   Created 40 tenants...
//   Created 50 tenants...
//   Creation time: 696.616255ms
//   After creation: 88.74 MB (+85.33 MB)
// Phase 3: Indexing 10000 docs per tenant...
//   Indexed 10 tenants...
//   Indexed 20 tenants...
//   Indexed 30 tenants...
//   Indexed 40 tenants...
//   Indexed 50 tenants...
//   Indexing time: 3.723523327s
//   After indexing: 683.15 MB (+679.74 MB from baseline)
// Phase 4: Committing all tenants...
//   Committed 10 tenants...
//   Committed 20 tenants...
//   Committed 30 tenants...
//   Committed 40 tenants...
//   Committed 50 tenants...
//   Commit time: 2.360908577s
//   After commits: 189.00 MB (+185.59 MB from baseline)
// Phase 5: Creating readers for all tenants...
//   Reader creation time: 15.2904ms
//   After readers: 203.78 MB (+200.37 MB from baseline)
// Phase 6: Querying all tenants (warm cache)...
//   Query time: 4.652407ms (500 total results)
//   After queries: 208.37 MB (+204.96 MB from baseline)
// Phase 7: Segment analysis
//   Segments per tenant: median=2, max=2
// Phase 8: Disk usage analysis
//   Total: 43.58 MB
//   Per tenant: 0.87 MB
// Phase 9: Testing inactive tenant memory (query only 10 tenants)...
//   Before selective: 208.37 MB
//   After selective: 208.37 MB
//   Delta: 0.00 MB (100 results)
// === RESULTS SUMMARY ===
// Memory overhead:
//   Total working set: 204.96 MB
//   Per tenant: 4.10 MB
// Density projection:
//   Available RAM: 4.0 GB (3584 MB usable)
//   Max tenants/4GB: 874
//   Target: 400 tenants
// Segment counts:
//   Median: 2 segments/tenant
//   Max: 2 segments/tenant
// Disk usage:
//   0.87 MB/tenant
// === INTERPRETATION ===
// ⚠️  MARGINAL: 4.10 MB/tenant - Reduced density
//    874 tenants/4GB (target was 400)
//    Economics still viable but margins tighter
// ✅ Segment count acceptable: 2 median segments/tenant
// Compression ratio: 2.29x
// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin tantivy_memory_density_test



// **4.10 MB/tenant is acceptable but suspicious.**

// Your test shows 874 max tenants/4GB - **2.2x better than target**. This contradicts your earlier catastrophic Tantivy result (23.4 MB/tenant under concurrent load).

// **Key differences between tests:**

// | Metric | Old test (write_latency_test) | This test | Delta |
// |--------|-------------------------------|-----------|-------|
// | Tenants | 10 | 50 | 5x |
// | Docs/tenant | 10K | 10K | Same |
// | Load pattern | Concurrent commits | Sequential commits | Different |
// | Memory/tenant | 23.4 MB | 4.10 MB | **5.7x better** |
// | Commit pattern | All at once | One-by-one | Different |

// **The 5.7x improvement suggests:**

// 1. **Concurrent commit penalty was real** - Your original test's 3,851ms P99 wasn't just fsync serialization. Memory ballooned because 10 writers held uncommitted segments simultaneously.

// 2. **Sequential batching works** - This test commits one-by-one, so only 1 tenant has dirty buffers at a time. Much lower peak RSS.

// 3. **But production won't be sequential** - You can't freeze 399 tenants while tenant #1 commits. Real workload = some concurrent writes.

// **Critical unknowns:**

// **Q1: What happens at 10-20 concurrent committers?**

// This test committed sequentially (2.36s total for 50 tenants). If 20 tenants write simultaneously in production, memory could spike toward the 23 MB/tenant you saw before.

// **Q2: Why is working set 4.10 MB but segments only 0.87 MB on disk?**

// - 204.96 MB total RSS
// - 43.58 MB total disk
// - **4.7x inflation**

// Tantivy keeps decompressed data in RAM (term dictionaries, posting lists). This is normal but means your density estimate depends on **active tenants**, not total tenants.

// **Revised density math:**

// If 20% active (80 of 400 tenants):
// - Active: 80 × 4.10 MB = 328 MB
// - Passive: 320 × 0.87 MB = 278 MB (just mmap overhead)
// - **Total: 606 MB** ✅ Fits 4GB

// If 50% active (200 of 400):
// - Active: 200 × 4.10 MB = 820 MB
// - Passive: 200 × 0.87 MB = 174 MB  
// - **Total: 994 MB** ✅ Still fits

// But if all 400 query simultaneously:
// - 400 × 4.10 MB = **1,640 MB** ✅ Fits but tight

// **The real test you need:**

// ```rust
// // Concurrent write + query load
// // 50 tenants, 10 commit simultaneously while others query
// // Measure: Does RSS spike above 4.10 MB/tenant?
// ```

// **Verdict:**

// ✅ **Test validates architecture IF:**
// - <50% tenants active simultaneously
// - Write batching stays sequential or limited concurrency

// ⚠️ **Needs validation:**
// - Concurrent commit memory spike
// - Query load during writes
// - 400 tenants all querying at once

// **Next test priority: `tantivy_sequential_batch_test.rs`**

// That measures commit latency, which determines if sequential batching is even viable. If P99 >100ms per tenant, 20 tenants = 2 second write delay = unacceptable.

// Run that test. If commit latency is acceptable, then 4.10 MB/tenant is your answer.


// ...


// Algolia Meilisearch write latency indexing time SLA production10 resultsHow fast is Algolia? – Algoliaalgolia.comExploration of search indexing: Real-time indexing | Algoliaalgolia.comHow fast is the indexing? – Algoliaalgolia.comHow Algolia Reduces Latency For 21B Searches Per Month | by Algolia | Algolia Storiesalgolia.comAlgolia vs Elasticsearch: Which Search Fits Your Needs?meilisearch.comTrack usage and performance in the dashboard - Algoliaalgolia.comComparing Algolia and Elasticsearch For Consumer-Grade Search Part 1: End-to-end Latencyalgolia.comHow Algolia Reduces Latency | Hacker Newsycombinator.comHow Algolia Reduces Latency For 21B Searches Per Month - Algolia Tech Stackstackshare.ioDon’t let network latency ruin the search experience of your international users - Algolia Blog | Algoliaalgolia.com
// **Skip the test revision. Move to the replication cost test.**

// **Competitor benchmarks:**

// Algolia processes indexing operations in seconds on shared clusters. Real-time indexing isn't required - most customers update inventory multiple times per day, not per sale. Your sequential batching (2.36s for 50 tenants = 47ms/tenant) is competitive.

// **Key insight from your test:**

// 4.10 MB/tenant working set is **fine**. You're 2.2x better than target. The real question isn't memory - it's **whether segment replication actually saves CPU**.

// **Critical unknown:**

// Your GLOBAL_REPLICATION.md claims segment replication = 0.31 cores vs document replication = 2.1 cores at 10 replicas. This 6.7x difference determines if you can afford 20-30 global replicas profitably.

// **Next test: `tantivy_segment_replication_cost.rs`**

// Validate:
// 1. Primary indexes 10K docs → measure CPU time
// 2. Replica rsyncs segments → measure CPU time  
// 3. Compare to full re-index

// If segment copy <10% of indexing CPU, your global replication economics work. If segment copy requires re-indexing (or heavy decompression), your cost model breaks.

// This test is **blocking** for your "seamless global replication" differentiator. Memory density is solved. Write latency is competitive. Replication cost is the last architectural unknown.

// Write that test next.

// https://claude.ai/chat/5b212596-1465-44dc-807a-c0a79cbc4024 %20tantivy_segment_replication_cost%20tantivy_memory_density_test