// src/bin/fst_overhead_test.rs
use lmdb::{Database, Environment, Transaction, WriteFlags};
use std::collections::HashMap;

// Add to Cargo.toml dependencies:
// fst = "0.4"

fn get_rss_mb() -> f64 {
    #[cfg(target_os = "macos")]
    {
        use mach2::mach_types::task_info_t;
        use mach2::task::{task_info, TASK_VM_INFO};
        use mach2::task_info::{task_vm_info, TASK_VM_INFO_COUNT};
        use mach2::traps::mach_task_self;
        use std::mem;

        unsafe {
            let mut info: task_vm_info = mem::zeroed();
            let mut count = TASK_VM_INFO_COUNT;
            let result = task_info(
                mach_task_self(),
                TASK_VM_INFO,
                &mut info as *mut task_vm_info as task_info_t,
                &mut count,
            );
            if result == 0 {
                return info.phys_footprint as f64 / 1024.0 / 1024.0;
            }
        }
    }
    
    #[cfg(not(target_os = "macos"))]
    {
        // Linux fallback - read from /proc/self/status
        if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(kb_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = kb_str.parse::<f64>() {
                            return kb / 1024.0;
                        }
                    }
                }
            }
        }
    }
    
    0.0
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

fn build_index(doc_count: u32) -> HashMap<String, Vec<u32>> {
    let mut index = HashMap::new();
    for doc_id in 0..doc_count {
        for term in tokenize(&generate_product(doc_id)) {
            index.entry(term).or_default().push(doc_id);
        }
    }
    index
}

fn main() {
    println!("=== FST Overhead Test ===\n");
    
    let path = "/tmp/lmdb_fst_test";
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
    
    let tenant_count = 20;
    let docs_per_tenant = 10_000;
    
    // Phase 1: Baseline (no FST)
    println!("Phase 1: Baseline LMDB storage (term strings as keys)");
    
    let env = Environment::new()
        .set_max_dbs(tenant_count)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(std::path::Path::new(path))
        .unwrap();
    
    let mut dbs = Vec::new();
    for tenant_id in 0..tenant_count {
        let db = env.create_db(
            Some(&format!("t{}_base", tenant_id)),
            lmdb::DatabaseFlags::empty()
        ).unwrap();
        
        let index = build_index(docs_per_tenant);
        let mut txn = env.begin_rw_txn().unwrap();
        for (term, postings) in index {
            let bytes: Vec<u8> = postings.iter().flat_map(|id| id.to_le_bytes()).collect();
            txn.put(db, &term.as_bytes(), &bytes, WriteFlags::empty()).unwrap();
        }
        txn.commit().unwrap();
        dbs.push(db);
    }
    
    // Query all to ensure working set resident
    {
        let txn = env.begin_ro_txn().unwrap();
        for db in &dbs {
            for term in ["laptop", "premium", "wireless"] {
                let _ = txn.get(*db, &term.as_bytes());
            }
        }
    }
    
    let rss_baseline = get_rss_mb();
    println!("Baseline RSS (20 DBs, all queried): {:.1} MB\n", rss_baseline);
    
    println!("Phase 2: Adding FST (term → ordinal mapping)");
    
    use fst::{Map, MapBuilder};
    let mut fst_maps = Vec::new();
    let mut fst_dbs = Vec::new();
    
    for (tenant_id, base_db) in dbs.iter().enumerate() {
        let mut terms = Vec::new();
        {
            let txn = env.begin_ro_txn().unwrap();
            let mut cursor = txn.open_ro_cursor(*base_db).unwrap();
            for (key, _) in cursor.iter() {
                terms.push(String::from_utf8_lossy(key).to_string());
            }
        }
        terms.sort();
        
        let mut builder = MapBuilder::memory();
        for (ordinal, term) in terms.iter().enumerate() {
            builder.insert(term, ordinal as u64).unwrap();
        }
        let fst = Map::new(builder.into_inner().unwrap()).unwrap();
        
        let fst_db = env.create_db(
            Some(&format!("t{}_fst", tenant_id)),
            lmdb::DatabaseFlags::empty()
        ).unwrap();
        
        let mut txn = env.begin_rw_txn().unwrap();
        for (ordinal, term) in terms.iter().enumerate() {
            let base_txn = env.begin_ro_txn().unwrap();
            if let Ok(postings) = base_txn.get(*base_db, &term.as_bytes()) {
                txn.put(fst_db, &(ordinal as u32).to_le_bytes(), &postings, WriteFlags::empty()).unwrap();
            }
        }
        txn.commit().unwrap();
        
        fst_maps.push(fst);
        fst_dbs.push(fst_db);
        
        if tenant_id % 5 == 4 {
            println!("  Built FST for tenant {}", tenant_id);
        }
    }
    
    {
        let txn = env.begin_ro_txn().unwrap();
        for (fst, db) in fst_maps.iter().zip(&fst_dbs) {
            for term in ["laptop", "premium", "wireless"] {
                if let Some(ordinal) = fst.get(term) {
                    let _ = txn.get(*db, &(ordinal as u32).to_le_bytes());
                }
            }
        }
    }
    
    let rss_with_fst = get_rss_mb();
    let fst_overhead = rss_with_fst - rss_baseline;
    
    println!("\n=== Results ===");
    println!("LMDB-only: {:.1} MB / 20 = {:.1} MB/tenant", rss_baseline, rss_baseline / 20.0);
    println!("With FST: {:.1} MB / 20 = {:.1} MB/tenant", rss_with_fst, rss_with_fst / 20.0);
    println!("FST overhead: {:.1} MB total = {:.1} MB/tenant", fst_overhead, fst_overhead / 20.0);
    
    if fst_overhead / 20.0 < 5.0 {
        println!("\n✓ FST overhead acceptable (<5 MB/tenant)");
    } else if fst_overhead / 20.0 < 10.0 {
        println!("\n⚠ FST overhead marginal (5-10 MB/tenant)");
    } else {
        println!("\n✗ FST overhead high (>10 MB/tenant)");
    }
}