use lmdb::{Database, Environment, Transaction, WriteFlags};
use std::time::Instant;

fn generate_term_postings(batch_size: u32, tenant_id: u32) -> Vec<(String, Vec<u8>)> {
    (0..batch_size)
        .map(|i| {
            let term = format!("term_t{}_b{}", tenant_id, i);
            let postings: Vec<u8> = (0..10).flat_map(|doc_id| (doc_id as u32).to_le_bytes()).collect();
            (term, postings)
        })
        .collect()
}

fn main() {
    println!("=== Write Batch Scaling Test ===\n");
    
    let path = "/tmp/lmdb_batch_scaling";
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
    
    let test_cases = [
        (10, 10),    // 10 tenants × 10 items = 100 items
        (10, 100),   // 10 tenants × 100 items = 1000 items
        (50, 20),    // 50 tenants × 20 items = 1000 items
        (100, 10),   // 100 tenants × 10 items = 1000 items
        (100, 100),  // 100 tenants × 100 items = 10000 items
    ];
    
    for (num_tenants, items_per_tenant) in test_cases {
        let env = Environment::new()
            .set_max_dbs(num_tenants)
            .set_map_size(10 * 1024 * 1024 * 1024)
            .open(std::path::Path::new(path))
            .unwrap();
        
        let dbs: Vec<Database> = (0..num_tenants)
            .map(|i| env.create_db(Some(&format!("t{}", i)), lmdb::DatabaseFlags::empty()).unwrap())
            .collect();
        
        let mut latencies = Vec::new();
        
        for iteration in 0..20 {
            let mut batch = Vec::new();
            for tenant_id in 0..num_tenants {
                let items = generate_term_postings(items_per_tenant, tenant_id + iteration * 1000);
                batch.push((tenant_id as usize, items));
            }
            
            let start = Instant::now();
            let mut txn = env.begin_rw_txn().unwrap();
            
            for (tenant_idx, items) in batch {
                for (term, postings) in items {
                    txn.put(dbs[tenant_idx], &term.as_bytes(), &postings, WriteFlags::empty()).unwrap();
                }
            }
            
            txn.commit().unwrap();
            latencies.push(start.elapsed().as_micros());
        }
        
        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() * 95) / 100];
        let p99 = latencies[(latencies.len() * 99) / 100];
        
        let total_items = num_tenants * items_per_tenant;
        println!("{} tenants × {} items = {} total items/commit", num_tenants, items_per_tenant, total_items);
        println!("  P50: {:.1}ms", p50 as f64 / 1000.0);
        println!("  P95: {:.1}ms", p95 as f64 / 1000.0);
        println!("  P99: {:.1}ms", p99 as f64 / 1000.0);
        
        if p99 < 500_000 {
            println!("  ✓ <500ms: Can sustain {}+ writes/sec", total_items * 2);
        } else if p99 < 1_000_000 {
            println!("  ⚠ 500ms-1s: Marginal throughput");
        } else {
            println!("  ✗ >1s: Bottleneck at this scale");
        }
        println!();
        
        let _ = std::fs::remove_dir_all(path);
        std::fs::create_dir_all(path).unwrap();
    }
    
    println!("=== Analysis ===");
    println!("Success criteria:");
    println!("  1000 items/commit <500ms → 2000+ writes/sec system-wide");
    println!("  Latency flat across tenant counts → cross-tenant batching scales");
    println!("\nIf P99 >1s at 1000 items: LMDB encoding bottleneck, not fsync");
}