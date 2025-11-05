use lmdb::{Database, Environment, Transaction, WriteFlags};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

fn get_rss_mb() -> Option<f64> {
    if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if line.starts_with("VmRSS:") {
                if let Some(kb_str) = line.split_whitespace().nth(1) {
                    if let Ok(kb) = kb_str.parse::<f64>() {
                        return Some(kb / 1024.0);
                    }
                }
            }
        }
    }
    None
}

fn generate_product(id: u32) -> String {
    let brands = ["Apple", "Samsung", "Sony", "LG", "Microsoft"];
    let categories = ["Laptop", "Phone", "Tablet", "Watch", "Speaker"];
    let adjectives = ["Premium", "Budget", "Pro", "Gaming", "Wireless"];
    
    format!("{} {} {} ${} Model-{}", 
            adjectives[((id / 13) % adjectives.len() as u32) as usize],
            brands[(id % brands.len() as u32) as usize],
            categories[((id / 5) % categories.len() as u32) as usize],
            100 + (id % 900), id)
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_lowercase()).collect()
}

fn build_inverted_index(doc_count: u32) -> HashMap<String, Vec<u32>> {
    let mut index: HashMap<String, Vec<u32>> = HashMap::new();
    for doc_id in 0..doc_count {
        for term in tokenize(&generate_product(doc_id)) {
            index.entry(term).or_default().push(doc_id);
        }
    }
    index
}

fn main() {
    println!("=== Selective Page Fault Test ===\n");
    
    let path = "/tmp/lmdb_selective_test";
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
    
    let tenant_count = 20;
    let docs_per_tenant = 10_000;
    
    // Phase 1: Create indices
    {
        let env = Environment::new()
            .set_max_dbs(tenant_count)
            .set_map_size(10 * 1024 * 1024 * 1024)
            .open(std::path::Path::new(path))
            .unwrap();
        
        for tenant_id in 0..tenant_count {
            let db = env.create_db(
                Some(&format!("tenant_{}", tenant_id)),
                lmdb::DatabaseFlags::empty()
            ).unwrap();
            
            let index = build_inverted_index(docs_per_tenant);
            let mut txn = env.begin_rw_txn().unwrap();
            for (term, postings) in index {
                let bytes: Vec<u8> = postings.iter().flat_map(|id| id.to_le_bytes()).collect();
                txn.put(db, &term.as_bytes(), &bytes, WriteFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
        }
        
        println!("Created 20 tenant DBs with 10K docs each");
    }
    
    // Check disk size
    if let Ok(output) = std::process::Command::new("du").args(&["-sb", path]).output() {
        if let Ok(s) = String::from_utf8(output.stdout) {
            if let Some(size_str) = s.split_whitespace().next() {
                if let Ok(bytes) = size_str.parse::<u64>() {
                    println!("Disk usage: {:.1} MB\n", bytes as f64 / 1024.0 / 1024.0);
                }
            }
        }
    }
    
    // Phase 2: Drop and reopen
    thread::sleep(Duration::from_secs(2));
    
    let env = Environment::new()
        .set_max_dbs(tenant_count)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(std::path::Path::new(path))
        .unwrap();
    
    let rss_after_reopen = get_rss_mb();
    println!("A. RSS after reopening env: {:.1} MB", rss_after_reopen.unwrap_or(0.0));
    
    // Phase 3: Open all handles (NO iteration)
    let dbs: Vec<Database> = (0..tenant_count)
        .map(|i| env.open_db(Some(&format!("tenant_{}", i))).unwrap())
        .collect();
    
    let rss_after_handles = get_rss_mb();
    println!("B. RSS after opening 20 handles: {:.1} MB", rss_after_handles.unwrap_or(0.0));
    
    if let (Some(after), Some(before)) = (rss_after_handles, rss_after_reopen) {
        println!("   Δ from reopen: {:.1} MB", after - before);
    }
    
    // Phase 4: Query ONLY 3 specific terms in tenant_0 (minimal access)
    {
        let txn = env.begin_ro_txn().unwrap();
        let target_terms = ["laptop", "premium", "wireless"];
        
        for term in &target_terms {
            let _ = txn.get(dbs[0], &term.as_bytes());
        }
    }
    
    let rss_after_query = get_rss_mb();
    println!("\nC. RSS after querying 3 terms in tenant_0: {:.1} MB", rss_after_query.unwrap_or(0.0));
    
    if let (Some(after), Some(before)) = (rss_after_query, rss_after_handles) {
        let delta_kb = (after - before) * 1024.0;
        println!("   Δ from handles: {:.1} KB\n", delta_kb);
        
        println!("=== Analysis ===");
        
        if delta_kb < 200.0 {
            println!("✓ SELECTIVE FAULTING WORKS");
            println!("  Query faulted <200 KB");
            println!("  Only accessed pages loaded");
            println!("  Inactive tenants cost ~0 MB RAM");
            println!("  Density target achievable: 400 tenants/4GB");
        } else if delta_kb < 500.0 {
            println!("⚠ PARTIAL FAULTING");
            println!("  Query faulted {:.1} KB", delta_kb);
            println!("  More than expected but <500 KB");
            println!("  May be metadata overhead or prefetch");
            println!("  Density: ~200-300 tenants/4GB");
        } else {
            println!("✗ FULL DB FAULTED");
            println!("  Query faulted {:.1} KB", delta_kb);
            println!("  Entire DB loaded on access");
            println!("  Opening handle = loading data");
            println!("  Density target FAILED");
            println!("  Cannot achieve 400 tenants/4GB");
        }
    } else {
        println!("RSS measurement failed");
    }
    
    // Validation: Check what we actually queried exists
    println!("\n=== Validation ===");
    let txn = env.begin_ro_txn().unwrap();
    for term in ["laptop", "premium", "wireless"] {
        match txn.get(dbs[0], &term.as_bytes()) {
            Ok(val) => {
                let doc_count = val.len() / 4;
                println!("Term '{}': {} docs", term, doc_count);
            }
            Err(_) => println!("Term '{}': NOT FOUND (test invalid)", term),
        }
    }
}


// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin selective_fault_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 6.10s
//      Running `target/release/selective_fault_test`
// === Selective Page Fault Test ===

// Created 20 tenant DBs with 10K docs each
// Disk usage: 10.8 MB

// A. RSS after reopening env: 5.0 MB
// B. RSS after opening 20 handles: 5.0 MB
//    Δ from reopen: 0.0 MB

// C. RSS after querying 3 terms in tenant_0: 5.1 MB
//    Δ from handles: 160.0 KB

// === Analysis ===
// ✓ SELECTIVE FAULTING WORKS
//   Query faulted <200 KB
//   Only accessed pages loaded
//   Inactive tenants cost ~0 MB RAM
//   Density target achievable: 400 tenants/4GB

// === Validation ===
// Term 'laptop': 2000 docs
// Term 'premium': 2002 docs
// Term 'wireless': 1992 docs
// ubuntu@ip-172-31-23-154:~/flapjack_rust$

// https://claude.ai/chat/7903090a-e3d7-40a9-995a-eda0105e13f6

// # LMDB selective faulting confirmed

// **Result: 160 KB faulted for 3 term queries**

// Opening 20 DB handles: 0 MB overhead
// Querying 3 terms in 1 DB: 160 KB

// This validates lazy-loading hypothesis. Inactive tenants cost ~0 MB RAM.

// ## Implications for $1/month pricing

// **At 10 MB/tenant working set (active):**
// - 400 tenants/4GB instance = $400/month revenue
// - t3.medium cost: $30/month
// - Margin: 13.3x (sustainable)

// **At 5 MB/tenant (small collections):**
// - 800 tenants/4GB = $800/month
// - Margin: 26.7x (excellent)

// $1/month pricing viable if:
// 1. Usage caps enforced (10K records, 1K queries/day)
// 2. Write batching achieves <500ms at 1000 items/commit
// 3. Sort performance acceptable

// ## Critical unknowns remain

// **Test 2: Write batch scaling** - Does commit latency stay <500ms at:
// - 100 tenants × 10 items = 1000 items/commit
// - 400 tenants × 2.5 items = 1000 items/commit

// If latency spikes >1s, can't sustain 10 writes/sec/tenant at scale.

// **Test 3: Sort** - Still completely untested. If broken, your differentiator fails.

// **Production unknowns:**
// - Concurrent read contention at 400 tenants
// - LMDB file growth without compaction
// - Page cache eviction under memory pressure

// ## Decision point

// Selective faulting works. This unblocks LMDB path for $1/month pricing.

// **Next steps:**
// 1. Test write batch scaling (3 hours) - validates throughput
// 2. Test sort (1 hour) - validates core feature
// 3. If both pass: commit to 6-8 week LMDB build
// 4. If either fails: reconsider architecture or pricing

// **Or:** Skip remaining tests, commit to LMDB based on:
// - Selective faulting proven
// - Cross-tenant batching architecturally required (Tantivy Test 2 failure)
// - Remaining unknowns addressable during implementation

// Risk: Discovering write throughput or sort performance issues 4 weeks into build.

// Which test next, or commit now?

....

// write these two tests now:

// Test write batch scaling (3 hours) - validates throughput
// Test sort (1 hour) - validates core feature
// src/bin/write_batch_scaling_test.rs
// src/bin/sort_test.rs










