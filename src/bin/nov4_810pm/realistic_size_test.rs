// Test: Memory overhead with realistic tenant sizes
// Question: Does per-env overhead scale with corpus size?
//
// Previous test used 1K docs/tenant (tiny). Production needs 10K-100K docs.
// Validate that 0.067 MB overhead doesn't balloon at realistic sizes.
//
// Theory: Overhead should be constant (just file descriptors + metadata).
// Working set should scale with accessed data, not total corpus.
//
// Method:
// 1. Create 50 tenants with 100K docs each (~50-100 MB per tenant)
// 2. Measure per-env overhead (handles only)
// 3. Query 1 tenant → measure active working set
// 4. Compare to baseline test (1K docs)
//
// Success: Overhead still <0.5 MB, working set <2 MB
// Failure: Overhead scales linearly with corpus size

use lmdb::{Environment, Transaction, WriteFlags};
use std::fs;
use std::path::PathBuf;

fn get_rss_mb() -> f64 {
    let pid = std::process::id();
    let status = fs::read_to_string(format!("/proc/{}/status", pid))
        .expect("Failed to read /proc/pid/status");
    
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let kb: u64 = line.split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            return kb as f64 / 1024.0;
        }
    }
    panic!("VmRSS not found");
}

fn create_large_tenant(base_path: &str, tenant_id: usize, doc_count: usize) -> PathBuf {
    let tenant_path = PathBuf::from(format!("{}/tenant_{}", base_path, tenant_id));
    fs::create_dir_all(&tenant_path).unwrap();
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(200_000_000) // 200 MB per tenant
        .open(&tenant_path)
        .unwrap();
    
    let db = env.create_db(Some("docs"), lmdb::DatabaseFlags::empty()).unwrap();
    
    // Write in batches
    let batch_size = 1000;
    for batch_start in (0..doc_count).step_by(batch_size) {
        let mut txn = env.begin_rw_txn().unwrap();
        let batch_end = (batch_start + batch_size).min(doc_count);
        
        for i in batch_start..batch_end {
            let key = format!("doc_{:08}", i);
            // ~500 bytes per doc
            let value = format!(
                "{{\"id\":{},\"tenant\":{},\"title\":\"Document {}\",\"body\":\"{}\"}}",
                i, tenant_id, i,
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
                Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris. \
                Nisi ut aliquip ex ea commodo consequat duis aute irure dolor."
            );
            txn.put(db, &key, &value, WriteFlags::empty()).unwrap();
        }
        txn.commit().unwrap();
    }
    
    tenant_path
}

fn get_dir_size_mb(path: &str) -> f64 {
    let output = std::process::Command::new("du")
        .args(&["-sm", path])
        .output()
        .unwrap();
    let size_str = String::from_utf8_lossy(&output.stdout);
    size_str.split_whitespace()
        .next()
        .unwrap()
        .parse::<f64>()
        .unwrap_or(0.0)
}

fn main() {
    let base_path = "/tmp/flapjack_realistic_size_test";
    let num_tenants = 50;
    let docs_per_tenant = 100_000;
    
    // Cleanup
    let _ = fs::remove_dir_all(base_path);
    fs::create_dir_all(base_path).unwrap();
    
    println!("=== Realistic Tenant Size Memory Test ===\n");
    println!("Creating {} tenants with {} docs each...", num_tenants, docs_per_tenant);
    println!("This will take 2-3 minutes...\n");
    
    // Phase 1: Create tenants
    let tenant_paths: Vec<PathBuf> = (0..num_tenants)
        .map(|i| {
            print!(".");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            create_large_tenant(base_path, i, docs_per_tenant)
        })
        .collect();
    
    println!("\n\nTenants created.");
    
    let total_size_mb = get_dir_size_mb(base_path);
    let avg_tenant_size = total_size_mb / num_tenants as f64;
    println!("Total disk usage: {:.1} MB", total_size_mb);
    println!("Average per tenant: {:.1} MB", avg_tenant_size);
    
    // Phase 2: Baseline RSS
    println!("\n--- Phase 2: Baseline RSS ---");
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);
    
    // Phase 3: Open all environments
    println!("\n--- Phase 3: Open {} large environments ---", num_tenants);
    let mut envs: Vec<Environment> = Vec::new();
    
    for (i, path) in tenant_paths.iter().enumerate() {
        let env = Environment::new()
            .set_max_dbs(10)
            .set_flags(lmdb::EnvironmentFlags::READ_ONLY | lmdb::EnvironmentFlags::NO_TLS)
            .open(path)
            .unwrap();
        envs.push(env);
        
        if i % 10 == 0 {
            print!(".");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
        }
    }
    
    let after_open_rss = get_rss_mb();
    let open_overhead = after_open_rss - baseline_rss;
    let per_env_overhead = open_overhead / num_tenants as f64;
    
    println!("\n\nRSS after opening {} envs: {:.2} MB", num_tenants, after_open_rss);
    println!("Total overhead: {:.2} MB", open_overhead);
    println!("Per-environment overhead: {:.3} MB", per_env_overhead);
    
    println!("\nComparison to baseline test (1K docs/tenant):");
    println!("  Baseline per-env overhead: 0.067 MB");
    println!("  Current per-env overhead: {:.3} MB", per_env_overhead);
    println!("  Ratio: {:.1}x", per_env_overhead / 0.067);
    
    if per_env_overhead < 0.1 {
        println!("\n✅ EXCELLENT: Overhead constant regardless of corpus size");
    } else if per_env_overhead < 0.5 {
        println!("\n✅ ACCEPTABLE: Slight increase but still <0.5 MB");
    } else if per_env_overhead < 1.0 {
        println!("\n⚠️  WARNING: Overhead growing with corpus size");
    } else {
        println!("\n❌ FAIL: Overhead scales with data, not constant");
    }
    
    // Phase 4: Query single tenant (working set)
    println!("\n--- Phase 4: Active tenant working set ---");
    let before_query_rss = get_rss_mb();
    
    {
        let env_0 = &envs[0];
        let db = env_0.open_db(Some("docs")).unwrap();
        let txn = env_0.begin_ro_txn().unwrap();
        
        // Query 1000 random docs (1% of corpus)
        for i in (0..docs_per_tenant).step_by(100) {
            let key = format!("doc_{:08}", i);
            let _value = txn.get(db, &key).unwrap();
        }
    }
    
    let after_query_rss = get_rss_mb();
    let query_overhead = after_query_rss - before_query_rss;
    
    println!("RSS before query: {:.2} MB", before_query_rss);
    println!("RSS after querying 1000 docs: {:.2} MB", after_query_rss);
    println!("Working set: {:.2} MB", query_overhead);
    
    println!("\nComparison to baseline test:");
    println!("  Baseline working set (1K docs): 0.30 MB");
    println!("  Current working set (100K docs): {:.2} MB", query_overhead);
    println!("  Ratio: {:.1}x", query_overhead / 0.30);
    
    if query_overhead < 2.0 {
        println!("\n✅ Working set reasonable for 100K doc corpus");
    } else if query_overhead < 5.0 {
        println!("\n⚠️  Working set larger than expected but manageable");
    } else {
        println!("\n❌ Working set too large - will limit density");
    }
    
    // Phase 5: Query multiple tenants
    println!("\n--- Phase 5: Multi-tenant working set ---");
    let before_multi_rss = get_rss_mb();
    
    {
        for tenant_idx in [0, 10, 20, 30, 40] {
            let env = &envs[tenant_idx];
            let db = env.open_db(Some("docs")).unwrap();
            let txn = env.begin_ro_txn().unwrap();
            
            // Query 100 docs per tenant
            for i in (0..docs_per_tenant).step_by(1000) {
                let key = format!("doc_{:08}", i);
                let _value = txn.get(db, &key).unwrap();
            }
        }
    }
    
    let after_multi_rss = get_rss_mb();
    let multi_overhead = after_multi_rss - before_multi_rss;
    let per_active_tenant = multi_overhead / 5.0;
    
    println!("RSS before multi-tenant queries: {:.2} MB", before_multi_rss);
    println!("RSS after querying 5 tenants: {:.2} MB", after_multi_rss);
    println!("5 active tenants working set: {:.2} MB", multi_overhead);
    println!("Per active tenant: {:.2} MB", per_active_tenant);
    
    // Summary
    println!("\n=== SUMMARY ===");
    println!("Tenant size: {:.1} MB disk per tenant", avg_tenant_size);
    println!("Per-environment overhead: {:.3} MB", per_env_overhead);
    println!("Per active tenant working set: {:.2} MB", per_active_tenant);
    println!("Total per tenant (overhead + working set): {:.2} MB", per_env_overhead + per_active_tenant);
    
    println!("\nProjected for 400 tenants (100K docs each):");
    let baseline_400 = per_env_overhead * 400.0;
    let active_80 = baseline_400 + (per_active_tenant * 80.0);
    let active_400 = (per_env_overhead + per_active_tenant) * 400.0;
    
    println!("  Baseline (all envs open): {:.0} MB", baseline_400);
    println!("  With 80 active (20%): {:.0} MB", active_80);
    println!("  With 400 active (100%): {:.0} MB", active_400);
    
    if active_80 < 3500.0 {
        println!("\n✅ VIABLE: 400 tenants @ 100K docs fits in 4GB");
    } else if active_80 < 4096.0 {
        println!("\n⚠️  TIGHT: 400 tenants possible but little headroom");
    } else {
        println!("\n❌ NOT VIABLE: Need to reduce density or tenant size");
    }
    
    println!("\nScaling ratio vs baseline test:");
    println!("  Docs per tenant: 100x larger (1K → 100K)");
    println!("  Disk per tenant: {:.0}x larger", avg_tenant_size / 0.3);
    println!("  Per-env overhead: {:.1}x larger", per_env_overhead / 0.067);
    println!("  Working set: {:.1}x larger", per_active_tenant / 0.23);
    
    if per_env_overhead / 0.067 < 2.0 && per_active_tenant / 0.23 < 3.0 {
        println!("\n✅ CONCLUSION: Memory overhead sub-linear with corpus size");
        println!("   Architecture validated for production-scale tenants");
    } else {
        println!("\n⚠️  CONCLUSION: Memory scales with corpus size");
        println!("   May need to limit tenant size or reduce density");
    }
    
    // Cleanup
    println!("\nCleaning up...");
    drop(envs);
    let _ = fs::remove_dir_all(base_path);
}