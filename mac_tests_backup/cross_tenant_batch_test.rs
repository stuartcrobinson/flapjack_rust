use lmdb::{Database, Environment, Transaction, WriteFlags};
use std::time::Instant;

fn generate_writes(tenant_count: u32, writes_per_tenant: u32) -> Vec<(u32, Vec<(String, Vec<u8>)>)> {
    let mut result = Vec::new();
    for tenant_id in 0..tenant_count {
        let mut writes = Vec::new();
        for i in 0..writes_per_tenant {
            let term = format!("term_{}_{}", tenant_id, i);
            let postings: Vec<u8> = vec![tenant_id as u8, i as u8];
            writes.push((term, postings));
        }
        result.push((tenant_id, writes));
    }
    result
}

fn main() {
    println!("=== Cross-Tenant Batch Write Test ===\n");
    
    let path = "/tmp/lmdb_batch_test";
    let _ = std::fs::remove_dir_all(path);
    std::fs::create_dir_all(path).unwrap();
    
    let tenant_count = 20;
    
    let env = Environment::new()
        .set_max_dbs(tenant_count)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(std::path::Path::new(path))
        .unwrap();
    
    let dbs: Vec<Database> = (0..tenant_count)
        .map(|i| env.create_db(Some(&format!("tenant_{}", i)), lmdb::DatabaseFlags::empty()).unwrap())
        .collect();
    
    println!("Testing batched write across {} tenants", tenant_count);
    
    let writes_per_tenant = 10;
    let pending = generate_writes(tenant_count, writes_per_tenant);
    
    let start = Instant::now();
    let mut txn = env.begin_rw_txn().unwrap();
    
    let mut total_writes = 0;
    for (tenant_id, writes) in pending {
        let db = dbs[tenant_id as usize];
        for (term, postings) in writes {
            txn.put(db, &term.as_bytes(), &postings, WriteFlags::empty()).unwrap();
            total_writes += 1;
        }
    }
    
    txn.commit().unwrap();
    let latency = start.elapsed();
    
    println!("Batched {} writes across {} tenants: {:.1}ms", 
             total_writes, tenant_count, latency.as_secs_f64() * 1000.0);
    println!("Per-write amortized: {:.2}ms", 
             latency.as_secs_f64() * 1000.0 / total_writes as f64);
    
    println!("\n=== Comparison to Tantivy ===");
    println!("Tantivy Test 2 (10 concurrent commits): 3,851ms P99");
    println!("LMDB (single cross-tenant commit): {:.1}ms", latency.as_secs_f64() * 1000.0);
    
    if latency.as_millis() < 300 {
        println!("✓ LMDB enables sub-300ms cross-tenant batching");
        println!("  Can sustain 3-10 batches/sec = 600-2000 writes/sec");
    } else if latency.as_millis() < 500 {
        println!("⚠ Marginal: 500ms batching limits to 2 batches/sec");
    } else {
        println!("✗ Too slow for real-time requirements");
    }
}