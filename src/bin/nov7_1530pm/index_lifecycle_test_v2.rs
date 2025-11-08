use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use tempfile::TempDir;

fn get_rss_mb() -> f64 {
    let pid = std::process::id();
    let status_path = format!("/proc/{}/status", pid);
    
    match fs::read_to_string(&status_path) {
        Ok(contents) => {
            for line in contents.lines() {
                if line.starts_with("VmRSS:") {
                    // Format: "VmRSS:    3564 kB"
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<f64>() {
                            return kb / 1024.0; // Convert KB to MB
                        }
                    }
                }
            }
            eprintln!("Warning: Could not find VmRSS in /proc/{}/status", pid);
            eprintln!("Status file contents:\n{}", contents);
            0.0
        }
        Err(e) => {
            eprintln!("Error reading /proc/{}/status: {}", pid, e);
            0.0
        }
    }
}

fn create_tenant_index(base_path: &Path, tenant_id: u32, doc_count: usize) -> PathBuf {
    let tenant_path = base_path.join(format!("tenant_{}", tenant_id));
    fs::create_dir_all(&tenant_path).unwrap();

    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    schema_builder.add_u64_field("timestamp", INDEXED | STORED);
    schema_builder.add_u64_field("price", FAST);
    let schema = schema_builder.build();

    let index = Index::create_in_dir(&tenant_path, schema.clone()).unwrap();
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    for i in 0..doc_count {
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let timestamp = schema.get_field("timestamp").unwrap();
        let price = schema.get_field("price").unwrap();

        writer
            .add_document(doc!(
                title => format!("Document {} for tenant {}", i, tenant_id),
                body => format!("This is the body content with some searchable text for document {} in tenant {}", i, tenant_id),
                timestamp => (1700000000 + i) as u64,
                price => (i % 1000) as u64,
            ))
            .unwrap();
    }

    writer.commit().unwrap();
    tenant_path
}

fn test_all_open(tenant_paths: &[PathBuf]) -> Result<f64, String> {
    println!("\n=== TEST A: Open All Indexes Simultaneously ===");
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);
    
    if baseline_rss < 1.0 {
        return Err("RSS measurement appears broken (returned < 1 MB)".to_string());
    }

    let start = Instant::now();
    let mut indexes = Vec::new();

    for (i, path) in tenant_paths.iter().enumerate() {
        match Index::open_in_dir(path) {
            Ok(index) => {
                indexes.push(index);
                if (i + 1) % 50 == 0 {
                    let current_rss = get_rss_mb();
                    println!(
                        "  Opened {} indexes - RSS: {:.2} MB (+{:.2} MB)",
                        i + 1,
                        current_rss,
                        current_rss - baseline_rss
                    );
                }
            }
            Err(e) => {
                return Err(format!(
                    "Failed to open index {} after {} successes: {}",
                    i,
                    indexes.len(),
                    e
                ));
            }
        }
    }

    let duration = start.elapsed();
    let final_rss = get_rss_mb();
    let memory_delta = final_rss - baseline_rss;

    println!("\n✅ Successfully opened all {} indexes", indexes.len());
    println!("Total memory cost: {:.2} MB", memory_delta);
    println!("Per-index cost: {:.3} MB", memory_delta / indexes.len() as f64);
    println!("Time to open: {:?}", duration);

    std::thread::sleep(std::time::Duration::from_secs(1));
    let steady_rss = get_rss_mb();
    println!("Steady-state RSS: {:.2} MB", steady_rss);

    Ok(memory_delta)
}

fn test_lru_style(tenant_paths: &[PathBuf], cache_size: usize) -> Result<f64, String> {
    println!("\n=== TEST B: LRU-Style Access (cache_size={}) ===", cache_size);
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);
    
    if baseline_rss < 1.0 {
        return Err("RSS measurement appears broken (returned < 1 MB)".to_string());
    }

    let start = Instant::now();
    let mut cache: Vec<Option<Index>> = vec![None; cache_size];
    let mut cache_idx = 0;
    let mut max_rss = baseline_rss;

    for (i, path) in tenant_paths.iter().enumerate() {
        cache[cache_idx] = None;
        
        match Index::open_in_dir(path) {
            Ok(index) => {
                cache[cache_idx] = Some(index);
                cache_idx = (cache_idx + 1) % cache_size;

                let current_rss = get_rss_mb();
                if current_rss > max_rss {
                    max_rss = current_rss;
                }

                if (i + 1) % 50 == 0 {
                    println!(
                        "  Accessed {} indexes - RSS: {:.2} MB (+{:.2} MB, peak: +{:.2} MB)",
                        i + 1,
                        current_rss,
                        current_rss - baseline_rss,
                        max_rss - baseline_rss
                    );
                }
            }
            Err(e) => {
                return Err(format!("Failed to open index {}: {}", i, e));
            }
        }
    }

    let duration = start.elapsed();
    let final_rss = get_rss_mb();
    let memory_delta = max_rss - baseline_rss;

    println!("\n✅ Accessed all {} indexes with cache_size={}", tenant_paths.len(), cache_size);
    println!("Peak memory cost: {:.2} MB", memory_delta);
    println!("Expected if cache works: ~{:.2} MB", cache_size as f64 * 2.38);
    println!("Final RSS: {:.2} MB", final_rss);
    println!("Time to access all: {:?}", duration);

    std::thread::sleep(std::time::Duration::from_secs(1));
    let steady_rss = get_rss_mb();
    println!("Steady-state RSS: {:.2} MB (after cache settled)", steady_rss);

    Ok(memory_delta)
}

fn test_open_close_thrash(tenant_paths: &[PathBuf], iterations: usize) -> Result<(), String> {
    println!("\n=== TEST C: Open/Close Thrash ({} iterations) ===", iterations);
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    let start = Instant::now();
    let tenant_count = tenant_paths.len();

    for i in 0..iterations {
        let path = &tenant_paths[i % tenant_count];
        let _index = Index::open_in_dir(path)
            .map_err(|e| format!("Failed to open index at iteration {}: {}", i, e))?;
    }

    let duration = start.elapsed();
    let final_rss = get_rss_mb();

    println!("\n✅ Completed {} open/close cycles", iterations);
    println!("Final RSS: {:.2} MB (+{:.2} MB)", final_rss, final_rss - baseline_rss);
    println!("Time per open/close: {:?}", duration / iterations as u32);
    println!("Total time: {:?}", duration);

    Ok(())
}

fn main() {
    println!("==============================================");
    println!("Index Lifecycle & LRU Cache Necessity Test");
    println!("==============================================");
    
    // Test RSS measurement first
    println!("\n--- Testing RSS measurement ---");
    let initial_rss = get_rss_mb();
    println!("Initial RSS: {:.2} MB", initial_rss);
    
    if initial_rss < 1.0 {
        eprintln!("\n❌ ERROR: RSS measurement is broken!");
        eprintln!("Cannot proceed with memory tests.");
        eprintln!("Debug: Try running 'cat /proc/{}/status | grep VmRSS'", std::process::id());
        return;
    }
    
    println!("\nQuestion: Do we need LRU caching, or does OS handle everything?");
    println!("\nTest configuration:");
    println!("  - Create 200 tenant indexes");
    println!("  - 1,000 documents per tenant (small but realistic)");
    println!("  - Test A: Open all 200 simultaneously");
    println!("  - Test B: LRU-style access with cache_size=120");
    println!("  - Test C: Open/close thrash test");

    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    println!("\n--- Creating tenant indexes ---");
    let tenant_count = 200;
    let docs_per_tenant = 1_000;
    
    let mut tenant_paths = Vec::new();
    let create_start = Instant::now();
    
    for i in 0..tenant_count {
        if i % 20 == 0 {
            println!("Creating tenant {} (RSS: {:.2} MB)...", i, get_rss_mb());
        }
        let path = create_tenant_index(base_path, i, docs_per_tenant);
        tenant_paths.push(path);
    }
    
    let after_create_rss = get_rss_mb();
    println!("✅ Created {} tenants in {:?}", tenant_count, create_start.elapsed());
    println!("RSS after creation: {:.2} MB", after_create_rss);

    let test_a_result = test_all_open(&tenant_paths);
    let test_b_result = test_lru_style(&tenant_paths, 120);
    let test_c_result = test_open_close_thrash(&tenant_paths, 1000);

    println!("\n\n==============================================");
    println!("RESULTS SUMMARY");
    println!("==============================================");

    match &test_a_result {
        Ok(memory_cost) => {
            println!("\n✅ Test A: Open All Simultaneously");
            println!("   Memory cost: {:.2} MB", memory_cost);
            println!("   Per-index: {:.3} MB", memory_cost / tenant_count as f64);
            
            if *memory_cost > 2000.0 {
                println!("   ⚠️  Memory usage very high - LRU cache likely needed");
            } else if *memory_cost > 1000.0 {
                println!("   ⚠️  Memory usage high - LRU cache probably beneficial");
            } else {
                println!("   ℹ️  Memory usage moderate - OS caching may be sufficient");
            }
        }
        Err(e) => {
            println!("\n❌ Test A: Failed");
            println!("   Error: {}", e);
            println!("   ⚠️  Cannot open all indexes - LRU cache REQUIRED");
        }
    }

    match &test_b_result {
        Ok(memory_cost) => {
            println!("\n✅ Test B: LRU-Style Access (cache_size=120)");
            println!("   Memory cost: {:.2} MB", memory_cost);
            println!("   Expected: ~{:.2} MB (120 × 2.38 MB)", 120.0 * 2.38);
            
            if *memory_cost > 0.0 {
                let efficiency = (120.0 * 2.38) / memory_cost;
                println!("   Efficiency: {:.1}% of expected", efficiency * 100.0);
            }
            
            if *memory_cost < 400.0 {
                println!("   ✅ LRU approach keeps memory bounded");
            } else {
                println!("   ⚠️  Memory higher than expected - investigate");
            }
        }
        Err(e) => {
            println!("\n❌ Test B: Failed - {}", e);
        }
    }

    match &test_c_result {
        Ok(_) => println!("\n✅ Test C: Open/close lifecycle works correctly"),
        Err(e) => println!("\n❌ Test C: Failed - {}", e),
    }

    println!("\n==============================================");
    println!("CONCLUSION");
    println!("==============================================");

    match (&test_a_result, &test_b_result) {
        (Ok(all_open), Ok(lru_style)) => {
            if *all_open > *lru_style * 2.0 {
                println!("\n✅ LRU cache provides significant benefit:");
                println!("   All-open: {:.2} MB", all_open);
                println!("   LRU-style: {:.2} MB", lru_style);
                println!("   Savings: {:.2} MB ({:.1}%)", 
                    all_open - lru_style,
                    ((all_open - lru_style) / all_open) * 100.0
                );
                println!("\n   Recommendation: Use moka cache with capacity=120");
            } else {
                println!("\n⚠️  LRU cache provides minimal benefit:");
                println!("   All-open: {:.2} MB", all_open);
                println!("   LRU-style: {:.2} MB", lru_style);
                println!("   Savings: {:.2} MB", all_open - lru_style);
                println!("\n   Recommendation: OS caching may be sufficient");
                println!("   Consider: File descriptor limits may still require cache");
            }
        }
        (Err(_), Ok(_)) => {
            println!("\n✅ LRU cache is REQUIRED:");
            println!("   Cannot open all indexes simultaneously");
            println!("   But LRU-style access succeeds");
            println!("\n   Recommendation: Use moka cache - it's essential, not optional");
        }
        _ => {
            println!("\n❌ Tests inconclusive - review errors above");
        }
    }

    println!("\n==============================================\n");
}



// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin index_lifecycle_test_v2
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 41.82s
//      Running `target/release/index_lifecycle_test_v2`
// ==============================================
// Index Lifecycle & LRU Cache Necessity Test
// ==============================================

// --- Testing RSS measurement ---
// Initial RSS: 3.10 MB

// Question: Do we need LRU caching, or does OS handle everything?

// Test configuration:
//   - Create 200 tenant indexes
//   - 1,000 documents per tenant (small but realistic)
//   - Test A: Open all 200 simultaneously
//   - Test B: LRU-style access with cache_size=120
//   - Test C: Open/close thrash test

// --- Creating tenant indexes ---
// Creating tenant 0 (RSS: 3.35 MB)...
// Creating tenant 20 (RSS: 73.72 MB)...
// Creating tenant 40 (RSS: 90.89 MB)...
// Creating tenant 60 (RSS: 105.66 MB)...
// Creating tenant 80 (RSS: 106.42 MB)...
// Creating tenant 100 (RSS: 106.58 MB)...
// Creating tenant 120 (RSS: 107.54 MB)...
// Creating tenant 140 (RSS: 107.43 MB)...
// Creating tenant 160 (RSS: 107.44 MB)...
// Creating tenant 180 (RSS: 107.50 MB)...
// ✅ Created 200 tenants in 18.290890559s
// RSS after creation: 107.42 MB

// === TEST A: Open All Indexes Simultaneously ===
// Baseline RSS: 107.42 MB
//   Opened 50 indexes - RSS: 107.42 MB (+0.00 MB)
//   Opened 100 indexes - RSS: 107.42 MB (+0.00 MB)
//   Opened 150 indexes - RSS: 107.42 MB (+0.00 MB)
//   Opened 200 indexes - RSS: 107.42 MB (+0.00 MB)

// ✅ Successfully opened all 200 indexes
// Total memory cost: 0.00 MB
// Per-index cost: 0.000 MB
// Time to open: 8.227974ms
// Steady-state RSS: 107.42 MB

// === TEST B: LRU-Style Access (cache_size=120) ===
// Baseline RSS: 107.42 MB
//   Accessed 50 indexes - RSS: 107.42 MB (+0.00 MB, peak: +0.00 MB)
//   Accessed 100 indexes - RSS: 107.42 MB (+0.00 MB, peak: +0.00 MB)
//   Accessed 150 indexes - RSS: 107.42 MB (+0.00 MB, peak: +0.00 MB)
//   Accessed 200 indexes - RSS: 107.42 MB (+0.00 MB, peak: +0.00 MB)

// ✅ Accessed all 200 indexes with cache_size=120
// Peak memory cost: 0.00 MB
// Expected if cache works: ~285.60 MB
// Final RSS: 107.42 MB
// Time to access all: 10.264234ms
// Steady-state RSS: 107.42 MB (after cache settled)

// === TEST C: Open/Close Thrash (1000 iterations) ===
// Baseline RSS: 107.42 MB

// ✅ Completed 1000 open/close cycles
// Final RSS: 107.42 MB (+0.00 MB)
// Time per open/close: 37.108µs
// Total time: 37.108297ms


// ==============================================
// RESULTS SUMMARY
// ==============================================

// ✅ Test A: Open All Simultaneously
//    Memory cost: 0.00 MB
//    Per-index: 0.000 MB
//    ℹ️  Memory usage moderate - OS caching may be sufficient

// ✅ Test B: LRU-Style Access (cache_size=120)
//    Memory cost: 0.00 MB
//    Expected: ~285.60 MB (120 × 2.38 MB)
//    ✅ LRU approach keeps memory bounded

// ✅ Test C: Open/close lifecycle works correctly

// ==============================================
// CONCLUSION
// ==============================================

// ⚠️  LRU cache provides minimal benefit:
//    All-open: 0.00 MB
//    LRU-style: 0.00 MB
//    Savings: 0.00 MB

//    Recommendation: OS caching may be sufficient
//    Consider: File descriptor limits may still require cache

// ==============================================


// **Critical Finding: Opening Index objects costs ~0 MB of RSS**

// This is the key result. Let me explain what's actually happening:

// ## What the Test Shows

// 1. **Baseline after index creation**: 107 MB RSS
// 2. **After opening 200 Index objects**: 107 MB RSS (no change)
// 3. **Per-index open cost**: 0.000 MB

// ## What This Means

// When you call `Index::open_in_dir()`, Tantivy is **NOT** loading the index data into memory. It's only:
// - Opening file descriptors
// - Memory-mapping the files (doesn't consume RSS until pages are accessed)
// - Creating minimal metadata structures

// The actual memory consumption happens **on first query** when:
// - Pages are faulted in from the mmap
// - Query execution structures are built
// - Results are materialized

// ## Your 2.38 MB Measurement is Wrong

// The "2.38 MB per tenant" from your CONSTRAINTS.md is measuring **query working set**, not **open index cost**.

// This test proves: `Index::open()` ≈ free (in RSS terms)

// ## Implications for LRU Cache Decision

// **File descriptor limits still matter:**
// ```bash
// ulimit -n  # Check your fd limit
// ```

// Typical limits:
// - Default Linux: 1024 fds
// - Each Index: ~2-5 fds (depends on segment count)
// - 200 indexes × 3 fds avg = 600 fds ✅ fits

// **At 600 tenants:**
// - 600 × 3 fds avg = 1800 fds ❌ exceeds typical limit

// ## The Real Question

// You need to test: **What's the memory cost when indexes are ACTIVELY QUERIED?**

// Try this:
// ```rust
// // After opening all 200 indexes
// for index in &indexes {
//     let reader = index.reader()?;
//     let searcher = reader.searcher();
//     // Run a query to fault in pages
//     searcher.search(&query, &TopDocs::with_limit(10))?;
// }
// // NOW measure RSS
// ```

// ## Revised Understanding

// **LRU cache is needed for:**
// 1. ❌ ~~Reducing RSS of opened indexes~~ (opens are free)
// 2. ✅ File descriptor limits (above ~300 tenants)
// 3. ✅ Bounding **query working set** when indexes are hot
// 4. ✅ Explicitly closing unused readers/searchers

// **The 2.38 MB is per ACTIVE tenant, not per opened index.**

// ## Next Test Needed

// Modify the test to:
// 1. Open index
// 2. Create reader + searcher  
// 3. Execute query
// 4. Measure RSS **after query execution**

// That's when you'll see the 2.38 MB per tenant materialize.

// **Hypothesis**: LRU cache is essential, but for different reasons than assumed. It's about managing **active query state**, not open file handles.

// Want me to write that test?