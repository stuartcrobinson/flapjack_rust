use lmdb::{Database, Environment, Transaction, WriteFlags};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

fn get_rss_mb() -> f64 {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        
        let output = Command::new("ps")
            .args(&["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
            .output();
        
        if let Ok(output) = output {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(kb) = s.trim().parse::<f64>() {
                    return kb / 1024.0;
                }
            }
        }
    }
    
    #[cfg(target_os = "linux")]
    {
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
    
    let brand = brands[(id % brands.len() as u32) as usize];
    let category = categories[((id / 5) % categories.len() as u32) as usize];
    let adj = adjectives[((id / 13) % adjectives.len() as u32) as usize];
    
    format!("{} {} {} ${} Model-{}", adj, brand, category, 100 + (id % 900), id)
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|s| s.to_lowercase())
        .collect()
}

fn build_inverted_index(doc_count: u32) -> HashMap<String, Vec<u32>> {
    let mut index: HashMap<String, Vec<u32>> = HashMap::new();
    
    for doc_id in 0..doc_count {
        let text = generate_product(doc_id);
        for term in tokenize(&text) {
            index.entry(term).or_default().push(doc_id);
        }
    }
    
    index
}

fn main() {
    println!("=== LMDB mmap Demand-Paging Test ===\n");
    
    let path = "/tmp/lmdb_mmap_test";
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
    
    let tenant_count = 20;
    let docs_per_tenant = 10_000;
    
    let rss_baseline = get_rss_mb();
    println!("A. Baseline RSS: {:.1} MB", rss_baseline);
    
    {
        let env = Environment::new()
            .set_max_dbs(tenant_count)
            .set_map_size(10 * 1024 * 1024 * 1024)
            .open(std::path::Path::new(path))
            .unwrap();
        
        for tenant_id in 0..tenant_count {
            let db: Database = env.create_db(
                Some(&format!("tenant_{}_terms", tenant_id)),
                lmdb::DatabaseFlags::empty()
            ).unwrap();
            
            let index = build_inverted_index(docs_per_tenant);
            
            let mut txn = env.begin_rw_txn().unwrap();
            for (term, postings) in index {
                let postings_bytes: Vec<u8> = postings.iter()
                    .flat_map(|id| id.to_le_bytes())
                    .collect();
                txn.put(db, &term.as_bytes(), &postings_bytes, WriteFlags::empty()).unwrap();
            }
            txn.commit().unwrap();
            
            if tenant_id % 5 == 4 {
                println!("  Indexed tenant {}", tenant_id);
            }
        }
        
        let rss_after_write = get_rss_mb();
        println!("B. RSS after writing 20 DBs: {:.1} MB (+{:.1})", 
                 rss_after_write, rss_after_write - rss_baseline);
    }
    
    thread::sleep(Duration::from_secs(2));
    let rss_after_drop = get_rss_mb();
    println!("C. RSS after drop + sleep: {:.1} MB (Δ from baseline: {:.1})", 
             rss_after_drop, rss_after_drop - rss_baseline);
    
    let env = Environment::new()
        .set_max_dbs(tenant_count)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(std::path::Path::new(path))
        .unwrap();
    
    let rss_after_reopen = get_rss_mb();
    println!("D. RSS after reopening env: {:.1} MB (Δ from drop: {:.1})", 
             rss_after_reopen, rss_after_reopen - rss_after_drop);
    
    let dbs: Vec<Database> = (0..tenant_count)
        .map(|i| env.open_db(Some(&format!("tenant_{}_terms", i))).unwrap())
        .collect();
    
    let rss_after_handles = get_rss_mb();
    println!("E. RSS after opening 20 DB handles: {:.1} MB (Δ from reopen: {:.1})", 
             rss_after_handles, rss_after_handles - rss_after_reopen);
    
    {
        let txn = env.begin_ro_txn().unwrap();
        let test_terms = ["laptop", "premium", "wireless", "gaming", "phone"];
        
        for term in &test_terms {
            for _ in 0..20 {
                let _ = txn.get(dbs[0], &term.as_bytes());
            }
        }
    }
    
    let rss_after_first_query = get_rss_mb();
    println!("F. RSS after querying DB 0 only: {:.1} MB (Δ from handles: {:.1})", 
             rss_after_first_query, rss_after_first_query - rss_after_handles);
    
    {
        let txn = env.begin_ro_txn().unwrap();
        let test_terms = ["laptop", "premium", "wireless", "gaming", "phone"];
        
        for db in &dbs {
            for term in &test_terms {
                for _ in 0..20 {
                    let _ = txn.get(*db, &term.as_bytes());
                }
            }
        }
    }
    
    let rss_after_all_queries = get_rss_mb();
    println!("G. RSS after querying all 20 DBs: {:.1} MB (Δ from first: {:.1})", 
             rss_after_all_queries, rss_after_all_queries - rss_after_first_query);
    
    println!("\n=== Analysis ===");
    
    let handle_overhead = rss_after_handles - rss_after_reopen;
    let first_db_working_set = rss_after_first_query - rss_after_handles;
    let remaining_dbs_working_set = rss_after_all_queries - rss_after_first_query;
    
    println!("Opening DB handles overhead: {:.1} MB", handle_overhead);
    println!("First DB working set: {:.1} MB", first_db_working_set);
    println!("Remaining 19 DBs working set: {:.1} MB", remaining_dbs_working_set);
    println!("Avg working set per DB: {:.1} MB", 
             (first_db_working_set + remaining_dbs_working_set) / 20.0);
    
    if handle_overhead < 5.0 && first_db_working_set > 2.0 {
        println!("\n✓ mmap lazy-loading WORKS:");
        println!("  - Opening handles doesn't fault pages (<5 MB overhead)");
        println!("  - Querying faults pages (>{:.1} MB per DB)", first_db_working_set);
        println!("  - Inactive tenants cost ~0 MB RAM");
    } else if handle_overhead > 50.0 {
        println!("\n✗ mmap PREFAULTING detected:");
        println!("  - Opening handles loaded {:.1} MB", handle_overhead);
        println!("  - All data resident regardless of access");
        println!("  - Density argument FAILS");
    } else {
        println!("\n⚠ Ambiguous results - need Linux test");
    }
}