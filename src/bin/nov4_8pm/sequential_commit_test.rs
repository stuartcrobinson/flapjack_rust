// Test: Sequential per-tenant commit latency
// Question: Is fsync serialization a problem without concurrent writes?
//
// Background: Tantivy test showed 3,851ms P99 with 10 CONCURRENT tenant commits.
// But with batched writes (1 commit/sec per tenant), commits aren't concurrent.
// This tests whether SEQUENTIAL commits avoid the serialization penalty.
//
// Method:
// 1. Create 20 separate LMDB environments
// 2. Accumulate writes for each tenant over 1-second window
// 3. Commit tenants SEQUENTIALLY (not concurrently)
// 4. Measure total time and per-tenant P99
//
// Success criteria: 20 sequential commits in <200ms (avg 10ms each)
// Failure: >1000ms total (suggests LMDB overhead beyond fsync)

use lmdb::{Environment, Database, Transaction, WriteFlags};
use std::fs;
use std::time::{Duration, Instant};
use std::path::PathBuf;

fn get_rss_mb() -> f64 {
    let pid = std::process::id();
    if let Ok(status) = fs::read_to_string(format!("/proc/{}/status", pid)) {
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let kb: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
                return kb as f64 / 1024.0;
            }
        }
    }
    0.0
}

struct TenantEnv {
    env: Environment,
    db: Database,
    path: PathBuf,
}

fn create_tenant(base_path: &str, tenant_id: usize) -> TenantEnv {
    let path = PathBuf::from(format!("{}/tenant_{}", base_path, tenant_id));
    fs::create_dir_all(&path).unwrap();
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(50_000_000) // 50 MB
        .open(&path)
        .unwrap();
    
    let db = env.create_db(Some("docs"), lmdb::DatabaseFlags::empty()).unwrap();
    
    TenantEnv { env, db, path }
}

fn main() {
    let base_path = "/tmp/flapjack_sequential_commit_test";
    let num_tenants = 20;
    let writes_per_tenant = 50; // Simulates 1 sec of write accumulation
    
    // Cleanup
    let _ = fs::remove_dir_all(base_path);
    fs::create_dir_all(base_path).unwrap();
    
    println!("=== Sequential Per-Tenant Commit Test ===\n");
    println!("Creating {} tenant environments...", num_tenants);
    
    let tenants: Vec<TenantEnv> = (0..num_tenants)
        .map(|i| create_tenant(base_path, i))
        .collect();
    
    println!("Tenants created.\n");
    
    // Test 1: All tenants have writes (worst case)
    println!("--- Test 1: ALL tenants active (worst case) ---");
    println!("Each tenant: {} writes/commit", writes_per_tenant);
    
    let mut commit_times = Vec::new();
    let total_start = Instant::now();
    
    for (tenant_id, tenant) in tenants.iter().enumerate() {
        let mut txn = tenant.env.begin_rw_txn().unwrap();
        
        // Accumulate writes (simulating 1-sec batch)
        for i in 0..writes_per_tenant {
            let key = format!("doc_{}_{}", tenant_id, i);
            let value = format!("{{\"tenant\":{},\"doc\":{},\"timestamp\":{}}}", 
                tenant_id, i, Instant::now().elapsed().as_millis());
            txn.put(tenant.db, &key, &value, WriteFlags::empty()).unwrap();
        }
        
        // Commit (sequential, not concurrent)
        let commit_start = Instant::now();
        txn.commit().unwrap();
        let commit_duration = commit_start.elapsed();
        
        commit_times.push(commit_duration.as_micros() as f64 / 1000.0);
    }
    
    let total_duration = total_start.elapsed();
    
    // Calculate statistics
    commit_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = commit_times[commit_times.len() / 2];
    let p99 = commit_times[(commit_times.len() * 99) / 100];
    let avg = commit_times.iter().sum::<f64>() / commit_times.len() as f64;
    let total_ms = total_duration.as_millis();
    
    println!("\nCommit latency (ms):");
    println!("  Average: {:.2}", avg);
    println!("  P50: {:.2}", p50);
    println!("  P99: {:.2}", p99);
    println!("  Min: {:.2}", commit_times[0]);
    println!("  Max: {:.2}", commit_times[commit_times.len() - 1]);
    println!("\nTotal time for {} sequential commits: {} ms", num_tenants, total_ms);
    println!("Throughput: {:.0} commits/sec", (num_tenants as f64 / total_duration.as_secs_f64()));
    
    if total_ms < 200 {
        println!("\n✅ EXCELLENT: {} commits in {}ms (avg {:.1}ms/commit)", 
            num_tenants, total_ms, total_ms as f64 / num_tenants as f64);
        println!("   Sequential commits avoid fsync serialization penalty");
    } else if total_ms < 500 {
        println!("\n✅ ACCEPTABLE: {} commits in {}ms (avg {:.1}ms/commit)", 
            num_tenants, total_ms, total_ms as f64 / num_tenants as f64);
        println!("   Slight overhead but still <1s write visibility target");
    } else if total_ms < 1000 {
        println!("\n⚠️  BORDERLINE: {} commits in {}ms (avg {:.1}ms/commit)", 
            num_tenants, total_ms, total_ms as f64 / num_tenants as f64);
        println!("   May struggle with 100+ active tenants");
    } else {
        println!("\n❌ FAIL: {} commits in {}ms (avg {:.1}ms/commit)", 
            num_tenants, total_ms, total_ms as f64 / num_tenants as f64);
        println!("   Sequential commits still too slow - LMDB overhead beyond fsync?");
    }
    
    // Test 2: Sparse writes (only 25% of tenants active)
    println!("\n--- Test 2: SPARSE writes (25% tenants active) ---");
    
    let active_tenants = num_tenants / 4;
    let mut sparse_commit_times = Vec::new();
    let sparse_start = Instant::now();
    
    for tenant_id in (0..num_tenants).step_by(4) {
        let tenant = &tenants[tenant_id];
        let mut txn = tenant.env.begin_rw_txn().unwrap();
        
        for i in 0..writes_per_tenant {
            let key = format!("doc_sparse_{}_{}", tenant_id, i);
            let value = format!("{{\"tenant\":{},\"doc\":{}}}", tenant_id, i);
            txn.put(tenant.db, &key, &value, WriteFlags::empty()).unwrap();
        }
        
        let commit_start = Instant::now();
        txn.commit().unwrap();
        sparse_commit_times.push(commit_start.elapsed().as_micros() as f64 / 1000.0);
    }
    
    let sparse_total = sparse_start.elapsed();
    sparse_commit_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let sparse_avg = sparse_commit_times.iter().sum::<f64>() / sparse_commit_times.len() as f64;
    let sparse_p99 = sparse_commit_times[(sparse_commit_times.len() * 99) / 100];
    
    println!("\n{} active tenants:", active_tenants);
    println!("  Average commit: {:.2} ms", sparse_avg);
    println!("  P99 commit: {:.2} ms", sparse_p99);
    println!("  Total time: {} ms", sparse_total.as_millis());
    
    // Test 3: Concurrent commits for comparison
    println!("\n--- Test 3: CONCURRENT commits (anti-pattern) ---");
    println!("Testing what happens if we DON'T batch sequentially...\n");
    
    use std::thread;
    use std::sync::Arc;
    
    let concurrent_start = Instant::now();
    let handles: Vec<_> = (0..num_tenants).map(|tenant_id| {
        let path = PathBuf::from(format!("{}/tenant_{}", base_path, tenant_id));
        
        thread::spawn(move || {
            let env = Environment::new()
                .set_max_dbs(10)
                .open(&path)
                .unwrap();
            let db = env.open_db(Some("docs")).unwrap();
            
            let mut txn = env.begin_rw_txn().unwrap();
            for i in 0..writes_per_tenant {
                let key = format!("doc_concurrent_{}_{}", tenant_id, i);
                let value = format!("{{\"tenant\":{}}}", tenant_id);
                txn.put(db, &key, &value, WriteFlags::empty()).unwrap();
            }
            
            let commit_start = Instant::now();
            txn.commit().unwrap();
            commit_start.elapsed()
        })
    }).collect();
    
    let mut concurrent_times: Vec<f64> = handles.into_iter()
        .map(|h| h.join().unwrap().as_micros() as f64 / 1000.0)
        .collect();
    
    let concurrent_total = concurrent_start.elapsed();
    concurrent_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let concurrent_avg = concurrent_times.iter().sum::<f64>() / concurrent_times.len() as f64;
    let concurrent_p99 = concurrent_times[(concurrent_times.len() * 99) / 100];
    
    println!("Concurrent commits (all tenants at once):");
    println!("  Average: {:.2} ms", concurrent_avg);
    println!("  P99: {:.2} ms", concurrent_p99);
    println!("  Total time: {} ms", concurrent_total.as_millis());
    
    println!("\n=== COMPARISON ===");
    println!("Sequential:  {} ms total, {:.2} ms P99", total_ms, p99);
    println!("Concurrent:  {} ms total, {:.2} ms P99", concurrent_total.as_millis(), concurrent_p99);
    println!("Penalty:     {}x total time, {:.1}x P99 latency", 
        concurrent_total.as_millis() / total_ms.max(1),
        concurrent_p99 / p99.max(0.01));
    
    if concurrent_p99 < p99 * 2.0 {
        println!("\n✅ Concurrent penalty minimal - fsync serialization not severe");
    } else if concurrent_p99 < p99 * 5.0 {
        println!("\n⚠️  Concurrent penalty moderate - sequential batching helps");
    } else {
        println!("\n❌ Concurrent penalty severe - sequential batching REQUIRED");
    }
    
    // Cleanup
    println!("\nCleaning up...");
    drop(tenants);
    let _ = fs::remove_dir_all(base_path);
}