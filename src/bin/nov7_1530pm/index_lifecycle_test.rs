use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use tempfile::TempDir;

/// Get RSS memory in MB (macOS-specific)
#[cfg(target_os = "macos")]
fn get_rss_mb() -> f64 {
    use libc::{c_int, getpid, proc_pidinfo, proc_taskinfo, PROC_PIDTASKINFO};
    use std::mem;

    unsafe {
        let pid = getpid() as c_int;
        let mut info: proc_taskinfo = mem::zeroed();
        let size = mem::size_of::<proc_taskinfo>() as c_int;

        let ret = proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut _,
            size,
        );

        if ret == size {
            info.pti_resident_size as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn get_rss_mb() -> f64 {
    0.0
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

/// Test A: Open all N indexes simultaneously (no cache)
fn test_all_open(tenant_paths: &[PathBuf]) -> Result<f64, String> {
    println!("\n=== TEST A: Open All Indexes Simultaneously ===");
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

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

    // Keep indexes alive to measure steady-state memory
    std::thread::sleep(std::time::Duration::from_secs(1));
    let steady_rss = get_rss_mb();
    println!("Steady-state RSS: {:.2} MB", steady_rss);

    Ok(memory_delta)
}

/// Test B: LRU-style access (open, use, drop, open next)
fn test_lru_style(tenant_paths: &[PathBuf], cache_size: usize) -> Result<f64, String> {
    println!("\n=== TEST B: LRU-Style Access (cache_size={}) ===", cache_size);
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    let start = Instant::now();
    let mut cache: Vec<Option<Index>> = vec![None; cache_size];
    let mut cache_idx = 0;

    // Access all tenants in sequence, but only keep cache_size in memory
    for (i, path) in tenant_paths.iter().enumerate() {
        // Evict oldest from cache
        cache[cache_idx] = None; // Drop the old index
        
        // Load new index
        match Index::open_in_dir(path) {
            Ok(index) => {
                cache[cache_idx] = Some(index);
                cache_idx = (cache_idx + 1) % cache_size;

                if (i + 1) % 50 == 0 {
                    let current_rss = get_rss_mb();
                    println!(
                        "  Accessed {} indexes - RSS: {:.2} MB (+{:.2} MB)",
                        i + 1,
                        current_rss,
                        current_rss - baseline_rss
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
    let memory_delta = final_rss - baseline_rss;

    println!("\n✅ Accessed all {} indexes with cache_size={}", tenant_paths.len(), cache_size);
    println!("Peak memory cost: {:.2} MB", memory_delta);
    println!("Expected if cache works: ~{:.2} MB", cache_size as f64 * 2.38);
    println!("Time to access all: {:?}", duration);

    // Keep cache alive to measure steady-state
    std::thread::sleep(std::time::Duration::from_secs(1));
    let steady_rss = get_rss_mb();
    println!("Steady-state RSS: {:.2} MB", steady_rss);

    Ok(memory_delta)
}

/// Test C: Open/close thrash (measure cost of lifecycle)
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
        // Index drops immediately here
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
    println!("\nQuestion: Do we need LRU caching, or does OS handle everything?");
    println!("\nTest configuration:");
    println!("  - Create 200 tenant indexes");
    println!("  - 1,000 documents per tenant (small but realistic)");
    println!("  - Test A: Open all 200 simultaneously");
    println!("  - Test B: LRU-style access with cache_size=120");
    println!("  - Test C: Open/close thrash test");

    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create tenant indexes
    println!("\n--- Creating tenant indexes ---");
    let tenant_count = 200;
    let docs_per_tenant = 1_000;
    
    let mut tenant_paths = Vec::new();
    let create_start = Instant::now();
    
    for i in 0..tenant_count {
        if i % 20 == 0 {
            println!("Creating tenant {}...", i);
        }
        let path = create_tenant_index(base_path, i, docs_per_tenant);
        tenant_paths.push(path);
    }
    
    println!("✅ Created {} tenants in {:?}", tenant_count, create_start.elapsed());

    // Run tests
    let test_a_result = test_all_open(&tenant_paths);
    let test_b_result = test_lru_style(&tenant_paths, 120);
    let test_c_result = test_open_close_thrash(&tenant_paths, 1000);

    // Summary
    println!("\n\n==============================================");
    println!("RESULTS SUMMARY");
    println!("==============================================");

    match test_a_result {
        Ok(memory_cost) => {
            println!("\n✅ Test A: Open All Simultaneously");
            println!("   Memory cost: {:.2} MB", memory_cost);
            println!("   Per-index: {:.3} MB", memory_cost / tenant_count as f64);
            
            if memory_cost > 2000.0 {
                println!("   ⚠️  Memory usage very high - LRU cache likely needed");
            } else if memory_cost > 1000.0 {
                println!("   ⚠️  Memory usage high - LRU cache probably beneficial");
            } else {
                println!("   ℹ️  Memory usage moderate - OS caching may be sufficient");
            }
        }
        Err(ref e) => {
            println!("\n❌ Test A: Failed");
            println!("   Error: {}", e);
            println!("   ⚠️  Cannot open all indexes - LRU cache REQUIRED");
        }
    }

    match test_b_result {
        Ok(memory_cost) => {
            println!("\n✅ Test B: LRU-Style Access (cache_size=120)");
            println!("   Memory cost: {:.2} MB", memory_cost);
            println!("   Expected: ~{:.2} MB (120 × 2.38 MB)", 120.0 * 2.38);
            
            let efficiency = (120.0 * 2.38) / memory_cost;
            println!("   Efficiency: {:.1}% of expected", efficiency * 100.0);
            
            if memory_cost < 400.0 {
                println!("   ✅ LRU approach keeps memory bounded");
            } else {
                println!("   ⚠️  Memory higher than expected - investigate");
            }
        }
        Err(ref e) => {
            println!("\n❌ Test B: Failed - {}", e);
        }
    }

    match test_c_result {
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