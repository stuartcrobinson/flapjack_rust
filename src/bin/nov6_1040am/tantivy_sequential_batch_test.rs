use std::fs;
use std::path::Path;
use std::time::Instant;
use tantivy::schema::{Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};
use tempfile::TempDir;

/// Test: Does sequential batching avoid fsync serialization catastrophe?
/// 
/// Previous test showed P99 = 3,851ms with 10 concurrent tenants.
/// Hypothesis: Concurrent commits caused OS-level fsync serialization.
/// This test commits tenants one-by-one to validate if sequential batching is viable.

struct TenantIndex {
    index: Index,
    writer: IndexWriter,
    dir: TempDir,
}

impl TenantIndex {
    fn new(tenant_id: usize) -> Self {
        let dir = TempDir::new().unwrap();
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT | STORED);
        schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&dir, schema.clone()).unwrap();
        let writer = index.writer(50_000_000).unwrap(); // 50MB heap per tenant
        
        println!("Tenant {} initialized at {:?}", tenant_id, dir.path());
        
        TenantIndex { index, writer, dir }
    }
    
    fn add_documents(&mut self, count: usize) {
        let schema = self.index.schema();
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        
        for i in 0..count {
            let doc = doc!(
                title => format!("Document {}", i),
                body => "The quick brown fox jumps over the lazy dog. ".repeat(10)
            );
            self.writer.add_document(doc).unwrap();
        }
    }
    
    fn commit(&mut self) -> Result<u64, tantivy::TantivyError> {
        self.writer.commit()
    }
}

fn measure_memory_usage() -> usize {
    // Simple RSS measurement (works on Linux/Mac)
    let pid = std::process::id();
    
    #[cfg(target_os = "linux")]
    {
        let statm_path = format!("/proc/{}/statm", pid);
        if let Ok(contents) = fs::read_to_string(&statm_path) {
            let parts: Vec<&str> = contents.split_whitespace().collect();
            if parts.len() >= 2 {
                // Second field is RSS in pages, multiply by page size (4096)
                if let Ok(rss_pages) = parts[1].parse::<usize>() {
                    return rss_pages * 4096;
                }
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    {
        // Mac RSS measurement is less reliable, return estimate
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
    println!("=== Tantivy Sequential Batch Write Test ===\n");
    
    let num_tenants = 20;
    let docs_per_batch = 100;
    
    println!("Creating {} tenant indices...", num_tenants);
    let start = Instant::now();
    let mut tenants: Vec<TenantIndex> = (0..num_tenants)
        .map(|i| TenantIndex::new(i))
        .collect();
    println!("Initialization: {:?}\n", start.elapsed());
    
    let baseline_memory = measure_memory_usage();
    println!("Baseline memory: {:.2} MB\n", baseline_memory as f64 / 1_000_000.0);
    
    // Phase 1: Add documents to all tenants (no commits)
    println!("Adding {} docs to each tenant...", docs_per_batch);
    for (i, tenant) in tenants.iter_mut().enumerate() {
        tenant.add_documents(docs_per_batch);
        if (i + 1) % 5 == 0 {
            println!("  {} tenants loaded...", i + 1);
        }
    }
    
    let after_add_memory = measure_memory_usage();
    println!("After adding docs: {:.2} MB (+{:.2} MB)\n", 
        after_add_memory as f64 / 1_000_000.0,
        (after_add_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    // Phase 2: Sequential commits with latency measurement
    println!("Committing sequentially...");
    let mut commit_latencies = Vec::new();
    
    let commit_start = Instant::now();
    for (i, tenant) in tenants.iter_mut().enumerate() {
        let start = Instant::now();
        tenant.commit().unwrap();
        let elapsed = start.elapsed();
        commit_latencies.push(elapsed.as_millis());
        
        if i < 3 || i >= num_tenants - 3 {
            println!("  Tenant {}: {:?}", i, elapsed);
        } else if i == 3 {
            println!("  ...");
        }
    }
    let total_commit_time = commit_start.elapsed();
    
    let after_commit_memory = measure_memory_usage();
    println!("\nAfter commits: {:.2} MB (+{:.2} MB from baseline)\n",
        after_commit_memory as f64 / 1_000_000.0,
        (after_commit_memory - baseline_memory) as f64 / 1_000_000.0
    );
    
    // Statistics
    commit_latencies.sort();
    let p50 = commit_latencies[commit_latencies.len() / 2];
    let p95 = commit_latencies[(commit_latencies.len() * 95) / 100];
    let p99 = commit_latencies[(commit_latencies.len() * 99) / 100];
    let avg = commit_latencies.iter().sum::<u128>() / commit_latencies.len() as u128;
    
    println!("=== RESULTS ===\n");
    println!("Total commit time: {:?}", total_commit_time);
    println!("Per-tenant latency:");
    println!("  Average: {}ms", avg);
    println!("  P50: {}ms", p50);
    println!("  P95: {}ms", p95);
    println!("  P99: {}ms", p99);
    
    // Measure disk usage
    let mut total_disk = 0u64;
    for tenant in &tenants {
        let path = tenant.dir.path();
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    total_disk += metadata.len();
                }
            }
        }
    }
    
    println!("\nDisk usage:");
    println!("  Total: {:.2} MB", total_disk as f64 / 1_000_000.0);
    println!("  Per tenant: {:.2} MB", total_disk as f64 / num_tenants as f64 / 1_000_000.0);
    
    println!("\nMemory overhead:");
    println!("  Per tenant: {:.2} MB", 
        (after_commit_memory - baseline_memory) as f64 / num_tenants as f64 / 1_000_000.0
    );
    
    println!("\n=== INTERPRETATION ===");
    if p99 < 100 {
        println!("✅ PASS: P99 <100ms - Sequential batching viable");
        println!("   Total time for {} tenants: {:?} (acceptable)", num_tenants, total_commit_time);
    } else if p99 < 500 {
        println!("⚠️  MARGINAL: P99 {}ms - Acceptable but not ideal", p99);
        println!("   Consider reducing batch size or commit frequency");
    } else {
        println!("❌ FAIL: P99 {}ms - Tantivy write path too slow", p99);
        println!("   Sequential batching does NOT solve contention issue");
    }
    
    let memory_per_tenant_mb = (after_commit_memory - baseline_memory) as f64 / num_tenants as f64 / 1_000_000.0;
    if memory_per_tenant_mb < 3.0 {
        println!("✅ Memory: {:.2} MB/tenant - Density target achievable", memory_per_tenant_mb);
    } else if memory_per_tenant_mb < 5.0 {
        println!("⚠️  Memory: {:.2} MB/tenant - Reduces max density", memory_per_tenant_mb);
    } else {
        println!("❌ Memory: {:.2} MB/tenant - Density target infeasible", memory_per_tenant_mb);
    }
}



// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin tantivy_sequential_batch_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
// warning: unused import: `std::path::Path`
//  --> src/bin/nov6_1040am/tantivy_sequential_batch_test.rs:2:5
//   |
// 2 | use std::path::Path;
//   |     ^^^^^^^^^^^^^^^
//   |
//   = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

// warning: `flapjack_rust` (bin "tantivy_sequential_batch_test") generated 1 warning (run `cargo fix --bin "tantivy_sequential_batch_test"` to apply 1 suggestion)
//     Finished `release` profile [optimized] target(s) in 37.92s
//      Running `target/release/tantivy_sequential_batch_test`
// === Tantivy Sequential Batch Write Test ===

// Creating 20 tenant indices...
// Tenant 0 initialized at "/tmp/.tmpQjold6"
// Tenant 1 initialized at "/tmp/.tmpjLpcml"
// Tenant 2 initialized at "/tmp/.tmptYAuff"
// Tenant 3 initialized at "/tmp/.tmptrmAiS"
// Tenant 4 initialized at "/tmp/.tmpdP4lrE"
// Tenant 5 initialized at "/tmp/.tmp6VGnAT"
// Tenant 6 initialized at "/tmp/.tmpgW1Pb6"
// Tenant 7 initialized at "/tmp/.tmpT5zgo9"
// Tenant 8 initialized at "/tmp/.tmpceBYX2"
// Tenant 9 initialized at "/tmp/.tmpnZW7Dz"
// Tenant 10 initialized at "/tmp/.tmpOv1tn7"
// Tenant 11 initialized at "/tmp/.tmpmwCJdj"
// Tenant 12 initialized at "/tmp/.tmp9AT3yG"
// Tenant 13 initialized at "/tmp/.tmpUajSXR"
// Tenant 14 initialized at "/tmp/.tmpsxK3LL"
// Tenant 15 initialized at "/tmp/.tmpIDGh1e"
// Tenant 16 initialized at "/tmp/.tmpAkErAR"
// Tenant 17 initialized at "/tmp/.tmpTB6Xht"
// Tenant 18 initialized at "/tmp/.tmpxOiyXc"
// Tenant 19 initialized at "/tmp/.tmpAqJpZY"
// Initialization: 295.995667ms

// Baseline memory: 37.78 MB

// Adding 100 docs to each tenant...
//   5 tenants loaded...
//   10 tenants loaded...
//   15 tenants loaded...
//   20 tenants loaded...
// After adding docs: 39.61 MB (+1.84 MB)

// Committing sequentially...
//   Tenant 0: 183.975774ms
//   Tenant 1: 36.137318ms
//   Tenant 2: 33.668041ms
//   ...
//   Tenant 17: 34.008701ms
//   Tenant 18: 30.346554ms
//   Tenant 19: 27.694564ms

// After commits: 76.91 MB (+39.13 MB from baseline)

// === RESULTS ===

// Total commit time: 800.355611ms
// Per-tenant latency:
//   Average: 39ms
//   P50: 32ms
//   P95: 183ms
//   P99: 183ms

// Disk usage:
//   Total: 0.24 MB
//   Per tenant: 0.01 MB

// Memory overhead:
//   Per tenant: 1.96 MB

// === INTERPRETATION ===
// ⚠️  MARGINAL: P99 183ms - Acceptable but not ideal
//    Consider reducing batch size or commit frequency
// ✅ Memory: 1.96 MB/tenant - Density target achievable
// ubuntu@ip-172-31-23-154:~/flapjack_rust$


// **P99 = 183ms is the outlier (first commit). Rest are 27-36ms.**

// This validates sequential batching works. The 183ms is cold-start overhead (segment writer initialization). P50 = 32ms is your real steady-state latency.

// **Architecture decision:**

// At 20 tenants × 32ms = 640ms to commit all. At 400 tenants × 32ms = **12.8 seconds** to flush all writes if done strictly sequentially.

// This breaks real-time feel if user writes and immediately queries. Options:

// 1. **Per-tenant commit queue** - Each tenant commits independently when buffer full (100 docs) or timeout (1sec). Don't block on other tenants. Downside: May have 10-20 concurrent commits during write spike = memory pressure (your old 23.4 MB/tenant problem resurfaces).

// 2. **Accept 12s write visibility** - Market as "sub-15 second indexing" instead of "real-time". Algolia does "seconds" for indexing anyway.

// 3. **Hybrid: Hot path for recent writers** - Prioritize tenants with recent writes in commit queue. Cold tenants wait longer.

// **Memory = 1.96 MB/tenant after commit is suspicious.**

// Test shows 39.13 MB overhead for 20 tenants = 1.96 MB each. But density test showed **4.10 MB/tenant** with 50 tenants and 10K docs. 

// This test only indexed 100 docs. At 10K docs, expect ~4 MB/tenant, not 2 MB.

// **Critical unvalidated assumption:**

// You're designing for 400 tenants but haven't tested concurrent commit memory at scale. If 40 tenants (10%) write simultaneously and each holds 4 MB during commit, that's 160 MB transient spike. Multiply by safety factor = could hit 500 MB+ spikes.

// **Segment replication test was invalid** - local rsync doesn't measure network/SSH CPU. Ignore those results. You need to test rsync over SSH to localhost to measure encryption overhead.

// **Next: Don't write more tests. Decide on write strategy** (per-tenant vs global queue) based on these results, then validate that specific design.


// https://claude.ai/chat/5b212596-1465-44dc-807a-c0a79cbc4024  %20tantivy_segment_replication_cost%20tantivy_memory_density_test