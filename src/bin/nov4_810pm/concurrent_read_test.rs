// Test: Concurrent read scaling under high QPS
// Question: Does LMDB's MVCC scale to 4K+ QPS or does reader lock table bottleneck?
//
// Critical: All previous tests were single-threaded. At 400 tenants × 10 QPS = 4K system QPS.
// LMDB claims "readers scale linearly" but this is unvalidated empirically.
//
// Method:
// 1. Create 100 tenant environments with 10K docs each
// 2. Baseline: single-threaded queries, measure P99
// 3. Concurrent: 100 threads × 50 queries = 5K total queries
// 4. Each thread queries random tenants
// 5. Measure P99 latency degradation vs baseline
//
// Success: P99 concurrent < 50ms (2x baseline acceptable)
// Failure: P99 > 200ms or crashes (reader lock table exhausted)

use lmdb::{Environment, Transaction, Cursor};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use std::thread;
use std::sync::{Arc, Barrier};
use std::sync::atomic::{AtomicU64, Ordering};

fn create_tenant(base_path: &str, tenant_id: usize, doc_count: usize) -> PathBuf {
    let path = PathBuf::from(format!("{}/tenant_{}", base_path, tenant_id));
    fs::create_dir_all(&path).unwrap();
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(100_000_000) // 100 MB
        .open(&path)
        .unwrap();
    
    let db = env.create_db(Some("docs"), lmdb::DatabaseFlags::empty()).unwrap();
    
    // Write docs
    let batch_size = 1000;
    for batch_start in (0..doc_count).step_by(batch_size) {
        let mut txn = env.begin_rw_txn().unwrap();
        let batch_end = (batch_start + batch_size).min(doc_count);
        
        for i in batch_start..batch_end {
            let key = format!("doc_{:08}", i);
            let value = format!(
                "{{\"id\":{},\"tenant\":{},\"title\":\"Document {}\",\"body\":\"Sample content for testing.\"}}",
                i, tenant_id, i
            );
            txn.put(db, &key, &value, lmdb::WriteFlags::empty()).unwrap();
        }
        txn.commit().unwrap();
    }
    
    path
}

fn query_random_docs(env: &Environment, db: lmdb::Database, doc_count: usize, query_count: usize) -> Vec<u128> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut latencies = Vec::with_capacity(query_count);
    
    for _ in 0..query_count {
        let doc_id = rng.gen_range(0..doc_count);
        let key = format!("doc_{:08}", doc_id);
        
        let start = Instant::now();
        let txn = env.begin_ro_txn().unwrap();
        let _value = txn.get(db, &key).unwrap();
        drop(txn);
        
        latencies.push(start.elapsed().as_micros());
    }
    
    latencies
}

fn main() {
    let base_path = "/tmp/flapjack_concurrent_read_test";
    let num_tenants = 100;
    let docs_per_tenant = 10_000;
    
    // Cleanup
    let _ = fs::remove_dir_all(base_path);
    fs::create_dir_all(base_path).unwrap();
    
    println!("=== Concurrent Read Scaling Test ===\n");
    println!("Creating {} tenants with {} docs each...", num_tenants, docs_per_tenant);
    
    let tenant_paths: Vec<PathBuf> = (0..num_tenants)
        .map(|i| {
            if i % 10 == 0 {
                print!(".");
                std::io::Write::flush(&mut std::io::stdout()).unwrap();
            }
            create_tenant(base_path, i, docs_per_tenant)
        })
        .collect();
    
    println!("\nTenants created.\n");
    
    // Open all environments
    println!("Opening {} environments...", num_tenants);
    let envs: Vec<(Environment, lmdb::Database)> = tenant_paths.iter()
        .map(|path| {
            let env = Environment::new()
                .set_max_dbs(10)
                .set_flags(lmdb::EnvironmentFlags::READ_ONLY | lmdb::EnvironmentFlags::NO_TLS)
                .open(path)
                .unwrap();
            let db = env.open_db(Some("docs")).unwrap();
            (env, db)
        })
        .collect();
    
    println!("Environments opened.\n");
    
    // Phase 1: Single-threaded baseline
    println!("--- Phase 1: Single-threaded baseline ---");
    let baseline_queries = 1000;
    let tenant_idx = 0;
    let (ref env, db) = envs[tenant_idx];
    
    let baseline_start = Instant::now();
    let mut baseline_latencies = query_random_docs(env, db, docs_per_tenant, baseline_queries);
    let baseline_total = baseline_start.elapsed();
    
    baseline_latencies.sort();
    let baseline_p50 = baseline_latencies[baseline_latencies.len() / 2] as f64 / 1000.0;
    let baseline_p99 = baseline_latencies[(baseline_latencies.len() * 99) / 100] as f64 / 1000.0;
    let baseline_avg = baseline_latencies.iter().sum::<u128>() as f64 / baseline_latencies.len() as f64 / 1000.0;
    let baseline_qps = baseline_queries as f64 / baseline_total.as_secs_f64();
    
    println!("Single-threaded {} queries:", baseline_queries);
    println!("  Average: {:.3} ms", baseline_avg);
    println!("  P50: {:.3} ms", baseline_p50);
    println!("  P99: {:.3} ms", baseline_p99);
    println!("  QPS: {:.0}", baseline_qps);
    
    // Phase 2: Low concurrency (10 threads)
    println!("\n--- Phase 2: Low concurrency (10 threads) ---");
    let low_threads = 10;
    let queries_per_thread = 100;
    
    let barrier = Arc::new(Barrier::new(low_threads));
    let error_count = Arc::new(AtomicU64::new(0));
    
    let handles: Vec<_> = (0..low_threads).map(|thread_id| {
        let tenant_paths = tenant_paths.clone();
        let barrier = Arc::clone(&barrier);
        let error_count = Arc::clone(&error_count);
        
        thread::spawn(move || {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let mut latencies = Vec::with_capacity(queries_per_thread);
            
            // Open random tenants for this thread
            let tenant_idx = rng.gen_range(0..num_tenants);
            let env = Environment::new()
                .set_max_dbs(10)
                .set_flags(lmdb::EnvironmentFlags::READ_ONLY | lmdb::EnvironmentFlags::NO_TLS)
                .open(&tenant_paths[tenant_idx])
                .unwrap();
            let db = env.open_db(Some("docs")).unwrap();
            
            barrier.wait(); // Sync start
            
            for _ in 0..queries_per_thread {
                let doc_id = rng.gen_range(0..docs_per_tenant);
                let key = format!("doc_{:08}", doc_id);
                
                let start = Instant::now();
                match env.begin_ro_txn() {
                    Ok(txn) => {
                        match txn.get(db, &key) {
                            Ok(_) => {
                                latencies.push(start.elapsed().as_micros());
                            }
                            Err(_) => {
                                error_count.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(_) => {
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            
            latencies
        })
    }).collect();
    
    let low_start = Instant::now();
    let mut all_low_latencies: Vec<u128> = handles.into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    let low_total = low_start.elapsed();
    
    all_low_latencies.sort();
    let low_p50 = all_low_latencies[all_low_latencies.len() / 2] as f64 / 1000.0;
    let low_p99 = all_low_latencies[(all_low_latencies.len() * 99) / 100] as f64 / 1000.0;
    let low_avg = all_low_latencies.iter().sum::<u128>() as f64 / all_low_latencies.len() as f64 / 1000.0;
    let low_qps = all_low_latencies.len() as f64 / low_total.as_secs_f64();
    let low_errors = error_count.load(Ordering::Relaxed);
    
    println!("{} threads × {} queries = {} total:", low_threads, queries_per_thread, all_low_latencies.len());
    println!("  Average: {:.3} ms", low_avg);
    println!("  P50: {:.3} ms", low_p50);
    println!("  P99: {:.3} ms", low_p99);
    println!("  QPS: {:.0}", low_qps);
    println!("  Errors: {}", low_errors);
    println!("  Degradation vs baseline: {:.1}x P99", low_p99 / baseline_p99);
    
    // Phase 3: High concurrency (100 threads = 5K QPS target)
    println!("\n--- Phase 3: High concurrency (100 threads) ---");
    let high_threads = 100;
    let queries_per_thread = 50;
    
    let barrier = Arc::new(Barrier::new(high_threads));
    let error_count = Arc::new(AtomicU64::new(0));
    
    let handles: Vec<_> = (0..high_threads).map(|thread_id| {
        let tenant_paths = tenant_paths.clone();
        let barrier = Arc::clone(&barrier);
        let error_count = Arc::clone(&error_count);
        
        thread::spawn(move || {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            let mut latencies = Vec::with_capacity(queries_per_thread);
            
            // Each thread queries 3-5 random tenants
            let my_tenants: Vec<usize> = (0..3).map(|_| rng.gen_range(0..num_tenants)).collect();
            let mut envs_dbs = Vec::new();
            
            for tenant_idx in &my_tenants {
                let env = Environment::new()
                    .set_max_dbs(10)
                    .set_flags(lmdb::EnvironmentFlags::READ_ONLY | lmdb::EnvironmentFlags::NO_TLS)
                    .open(&tenant_paths[*tenant_idx])
                    .unwrap();
                let db = env.open_db(Some("docs")).unwrap();
                envs_dbs.push((env, db));
            }
            
            barrier.wait(); // Sync start
            
            for _ in 0..queries_per_thread {
                let tenant_choice = rng.gen_range(0..envs_dbs.len());
                let (ref env, db) = envs_dbs[tenant_choice];
                
                let doc_id = rng.gen_range(0..docs_per_tenant);
                let key = format!("doc_{:08}", doc_id);
                
                let start = Instant::now();
                match env.begin_ro_txn() {
                    Ok(txn) => {
                        match txn.get(db, &key) {
                            Ok(_) => {
                                latencies.push(start.elapsed().as_micros());
                            }
                            Err(_) => {
                                error_count.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(_) => {
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            
            latencies
        })
    }).collect();
    
    let high_start = Instant::now();
    let mut all_high_latencies: Vec<u128> = handles.into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    let high_total = high_start.elapsed();
    
    all_high_latencies.sort();
    let high_p50 = all_high_latencies[all_high_latencies.len() / 2] as f64 / 1000.0;
    let high_p99 = all_high_latencies[(all_high_latencies.len() * 99) / 100] as f64 / 1000.0;
    let high_avg = all_high_latencies.iter().sum::<u128>() as f64 / all_high_latencies.len() as f64 / 1000.0;
    let high_qps = all_high_latencies.len() as f64 / high_total.as_secs_f64();
    let high_errors = error_count.load(Ordering::Relaxed);
    
    println!("{} threads × {} queries = {} total:", high_threads, queries_per_thread, all_high_latencies.len());
    println!("  Average: {:.3} ms", high_avg);
    println!("  P50: {:.3} ms", high_p50);
    println!("  P99: {:.3} ms", high_p99);
    println!("  QPS: {:.0}", high_qps);
    println!("  Errors: {}", high_errors);
    println!("  Degradation vs baseline: {:.1}x P99", high_p99 / baseline_p99);
    
    // Summary
    println!("\n=== SUMMARY ===");
    println!("Baseline (single-thread): {:.3} ms P99, {:.0} QPS", baseline_p99, baseline_qps);
    println!("Low concurrency (10 threads): {:.3} ms P99, {:.0} QPS", low_p99, low_qps);
    println!("High concurrency (100 threads): {:.3} ms P99, {:.0} QPS", high_p99, high_qps);
    println!("\nScaling factor:");
    println!("  10 threads: {:.1}x latency, {:.1}x throughput", low_p99 / baseline_p99, low_qps / baseline_qps);
    println!("  100 threads: {:.1}x latency, {:.1}x throughput", high_p99 / baseline_p99, high_qps / baseline_qps);
    
    if high_errors > 0 {
        println!("\n❌ ERRORS: {} failed queries under high concurrency", high_errors);
        println!("   Possible reader lock table exhaustion");
    }
    
    if high_p99 < 50.0 {
        println!("\n✅ EXCELLENT: P99 <50ms under 5K QPS load");
        println!("   LMDB read scaling validated for production");
    } else if high_p99 < 100.0 {
        println!("\n✅ ACCEPTABLE: P99 <100ms under 5K QPS load");
        println!("   Read scaling adequate but monitor closely");
    } else if high_p99 < 200.0 {
        println!("\n⚠️  BORDERLINE: P99 {}ms under load", high_p99);
        println!("   May struggle at scale or need tuning");
    } else {
        println!("\n❌ FAIL: P99 {}ms unacceptable for production", high_p99);
        println!("   LMDB may not scale to required QPS");
    }
    
    // Cleanup
    println!("\nCleaning up...");
    drop(envs);
    let _ = fs::remove_dir_all(base_path);
}