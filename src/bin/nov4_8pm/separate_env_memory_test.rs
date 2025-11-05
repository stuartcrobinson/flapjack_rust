// Test: Does separate-file-per-tenant kill density economics?
// Question: What's the RSS overhead of opening 400 separate LMDB environments?
//
// Architecture decision: If separate files cost 10+ MB overhead per tenant,
// the 400 tenants/4GB pricing model fails. Need <1 MB overhead per env.
//
// Method:
// 1. Baseline RSS with no envs open
// 2. Open 400 separate LMDB environments (read-only)
// 3. Measure RSS delta (= per-env overhead)
// 4. Query 1 tenant → measure working set for active tenant
// 5. Query 10 random tenants → measure multi-tenant active working set

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

fn create_tenant_env(base_path: &str, tenant_id: usize, doc_count: usize) -> PathBuf {
    let tenant_path = PathBuf::from(format!("{}/tenant_{}", base_path, tenant_id));
    fs::create_dir_all(&tenant_path).unwrap();
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(100_000_000) // 100 MB per tenant
        .open(&tenant_path)
        .unwrap();
    
    let db = env.create_db(Some("docs"), lmdb::DatabaseFlags::empty()).unwrap();
    
    // Write sample docs
    let mut txn = env.begin_rw_txn().unwrap();
    for i in 0..doc_count {
        let key = format!("doc_{}", i);
        let value = format!("{{\"id\":{},\"title\":\"Document {}\",\"body\":\"Sample content for testing memory overhead. This is document number {} in tenant {}.\"}}",
            i, i, i, tenant_id);
        txn.put(db, &key, &value, WriteFlags::empty()).unwrap();
    }
    txn.commit().unwrap();
    
    tenant_path
}

fn main() {
    let base_path = "/tmp/flapjack_separate_env_test";
    let num_tenants = 400;
    let docs_per_tenant = 1000; // Small corpus to focus on overhead, not data
    
    // Cleanup
    let _ = fs::remove_dir_all(base_path);
    fs::create_dir_all(base_path).unwrap();
    
    println!("=== Separate Environment Memory Overhead Test ===\n");
    println!("Creating {} tenant environments with {} docs each...", num_tenants, docs_per_tenant);
    
    // Phase 1: Create tenant data
    let tenant_paths: Vec<PathBuf> = (0..num_tenants)
        .map(|i| {
            if i % 50 == 0 {
                print!(".");
                std::io::Write::flush(&mut std::io::stdout()).unwrap();
            }
            create_tenant_env(base_path, i, docs_per_tenant)
        })
        .collect();
    
    println!("\n\nDisk usage:");
    let output = std::process::Command::new("du")
        .args(&["-sh", base_path])
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    
    // Phase 2: Measure baseline RSS
    println!("\n--- Phase 2: Baseline RSS ---");
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);
    
    // Phase 3: Open all environments (read-only)
    println!("\n--- Phase 3: Open {} environments ---", num_tenants);
    let mut envs: Vec<Environment> = Vec::new();
    
    for (i, path) in tenant_paths.iter().enumerate() {
        let env = Environment::new()
            .set_max_dbs(10)
            .set_flags(lmdb::EnvironmentFlags::READ_ONLY | lmdb::EnvironmentFlags::NO_TLS)
            .open(path)
            .unwrap();
        envs.push(env);
        
        if i % 50 == 0 {
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
    
    if per_env_overhead > 1.0 {
        println!("\n❌ FAIL: Per-env overhead {:.3} MB exceeds 1 MB threshold", per_env_overhead);
        println!("   At 400 tenants: {:.2} MB baseline overhead alone", per_env_overhead * 400.0);
        println!("   This architecture is NOT viable for high-density hosting");
    } else if per_env_overhead > 0.5 {
        println!("\n⚠️  WARNING: Per-env overhead {:.3} MB is acceptable but tight", per_env_overhead);
        println!("   At 400 tenants: {:.2} MB baseline overhead", per_env_overhead * 400.0);
    } else {
        println!("\n✅ PASS: Per-env overhead {:.3} MB is acceptable", per_env_overhead);
        println!("   At 400 tenants: {:.2} MB baseline overhead", per_env_overhead * 400.0);
    }
    
    // Phase 4: Query single tenant (measure working set)
    println!("\n--- Phase 4: Query single tenant (working set) ---");
    let before_query_rss = get_rss_mb();
    
    {
        let env_0 = &envs[0];
        let db = env_0.open_db(Some("docs")).unwrap();
        let txn = env_0.begin_ro_txn().unwrap();
        
        // Query 100 random docs
        for i in (0..docs_per_tenant).step_by(10) {
            let key = format!("doc_{}", i);
            let _value = txn.get(db, &key).unwrap();
        }
    } // txn and db dropped here
    
    let after_query_rss = get_rss_mb();
    let query_overhead = after_query_rss - before_query_rss;
    
    println!("RSS before query: {:.2} MB", before_query_rss);
    println!("RSS after querying tenant 0: {:.2} MB", after_query_rss);
    println!("Single active tenant working set: {:.2} MB", query_overhead);
    
    // Phase 5: Query 10 random tenants
    println!("\n--- Phase 5: Query 10 random tenants ---");
    let before_multi_rss = get_rss_mb();
    
    {
        for tenant_idx in [5, 50, 100, 150, 200, 250, 300, 350, 380, 399] {
            let env = &envs[tenant_idx];
            let db = env.open_db(Some("docs")).unwrap();
            let txn = env.begin_ro_txn().unwrap();
            
            // Query 10 docs per tenant
            for i in (0..docs_per_tenant).step_by(100) {
                let key = format!("doc_{}", i);
                let _value = txn.get(db, &key).unwrap();
            }
        }
    } // all txns and dbs dropped
    
    let after_multi_rss = get_rss_mb();
    let multi_overhead = after_multi_rss - before_multi_rss;
    let per_active_tenant = multi_overhead / 10.0;
    
    println!("RSS before multi-tenant queries: {:.2} MB", before_multi_rss);
    println!("RSS after querying 10 tenants: {:.2} MB", after_multi_rss);
    println!("10 active tenants working set: {:.2} MB", multi_overhead);
    println!("Per active tenant: {:.2} MB", per_active_tenant);
    
    // Summary
    println!("\n=== SUMMARY ===");
    println!("Per-environment overhead (handles only): {:.3} MB", per_env_overhead);
    println!("Per active tenant working set: {:.2} MB", per_active_tenant);
    println!("Total per tenant (overhead + working set): {:.2} MB", per_env_overhead + per_active_tenant);
    println!("\nProjected for 400 tenants:");
    println!("  Baseline (all envs open): {:.2} MB", per_env_overhead * 400.0);
    println!("  With 80 active (20%): {:.2} MB", (per_env_overhead * 400.0) + (per_active_tenant * 80.0));
    println!("  With 400 active (100%): {:.2} MB", (per_env_overhead + per_active_tenant) * 400.0);
    
    let total_400_20pct = (per_env_overhead * 400.0) + (per_active_tenant * 80.0);
    if total_400_20pct > 4096.0 {
        println!("\n❌ CRITICAL: 400 tenants (20% active) exceeds 4GB target");
        println!("   Separate environments NOT viable for density target");
    } else if total_400_20pct > 3500.0 {
        println!("\n⚠️  WARNING: 400 tenants (20% active) uses {:.0} MB", total_400_20pct);
        println!("   Little headroom for OS, search structures, etc.");
    } else {
        println!("\n✅ VIABLE: 400 tenants (20% active) fits in 4GB with headroom");
    }
    
    // Cleanup
    println!("\nCleaning up...");
    drop(envs);
    let _ = fs::remove_dir_all(base_path);
}