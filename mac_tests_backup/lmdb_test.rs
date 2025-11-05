use lmdb::{Cursor, Environment, Transaction, WriteFlags};
use rand::Rng;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use std::collections::HashMap;

/// Test 1: Memory overhead per database with minimal data
fn test_empty_db_overhead(num_dbs: usize) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 1: Empty Database Overhead ===");
    println!("Creating {} empty LMDB databases...", num_dbs);
    
    let base_dir = PathBuf::from("/tmp/lmdb_test_empty");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir)?;
    
    let baseline_rss = get_rss_kb();
    println!("Baseline RSS: {} MB", baseline_rss / 1024);
    
    // Single LMDB environment with multiple named databases
    let env = Environment::new()
        .set_max_dbs(num_dbs as u32)
        .set_map_size(10 * 1024 * 1024 * 1024) // 10GB max
        .open(&base_dir)?;
    
    let mut databases = Vec::new();
    
    for i in 0..num_dbs {
        let db_name = format!("tenant_{}", i);
        let db = env.create_db(Some(&db_name), lmdb::DatabaseFlags::empty())?;
        databases.push(db);
        
        if (i + 1) % 10 == 0 {
            let current_rss = get_rss_kb();
            let overhead = current_rss - baseline_rss;
            println!(
                "  After {} databases: RSS = {} MB (+{} MB, ~{} MB/db)",
                i + 1,
                current_rss / 1024,
                overhead / 1024,
                overhead / 1024 / (i + 1)
            );
        }
    }
    
    let final_rss = get_rss_kb();
    let total_overhead = final_rss - baseline_rss;
    println!(
        "\nFinal: {} databases, {} MB total overhead, ~{} MB per database",
        num_dbs,
        total_overhead / 1024,
        total_overhead / 1024 / num_dbs
    );
    
    Ok(())
}

/// Test 2: Memory overhead with realistic inverted index data
fn test_realistic_inverted_index(num_dbs: usize, docs_per_db: usize) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 2: Realistic Inverted Index Overhead ===");
    println!("Creating {} databases with {}K docs each...", num_dbs, docs_per_db / 1000);
    
    let base_dir = PathBuf::from("/tmp/lmdb_test_realistic");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir)?;
    
    let baseline_rss = get_rss_kb();
    println!("Baseline RSS: {} MB", baseline_rss / 1024);
    
    let env = Environment::new()
        .set_max_dbs(num_dbs as u32)
        .set_map_size(50 * 1024 * 1024 * 1024) // 50GB max
        .open(&base_dir)?;
    
    let mut rng = rand::thread_rng();
    let start = Instant::now();
    
    for db_idx in 0..num_dbs {
        let db_name = format!("tenant_{}", db_idx);
        let db = env.create_db(Some(&db_name), lmdb::DatabaseFlags::empty())?;
        
        // Build simple inverted index: term -> posting list
        let mut inverted_index: HashMap<String, Vec<u32>> = HashMap::new();
        
        for doc_id in 0..docs_per_db {
            let title = generate_product_title(&mut rng);
            let body = generate_product_description(&mut rng);
            let text = format!("{} {}", title, body);
            
            // Tokenize and build inverted index
            for token in text.split_whitespace() {
                let term = token.to_lowercase();
                inverted_index.entry(term).or_insert_with(Vec::new).push(doc_id as u32);
            }
        }
        
        // Write inverted index to LMDB
        let mut txn = env.begin_rw_txn()?;
        for (term, postings) in inverted_index.iter() {
            // Encode posting list (simple delta encoding)
            let encoded = encode_posting_list(postings);
            txn.put(db, &term, &encoded, WriteFlags::empty())?;
        }
        txn.commit()?;
        
        // FORCE PAGE FAULTS: Read back all data to ensure it's in RSS
        let read_txn = env.begin_ro_txn()?;
        let mut cursor = read_txn.open_ro_cursor(db)?;
        let mut total_bytes = 0usize;
        for (_key, value) in cursor.iter() {
            total_bytes += value.len();
        }
        drop(cursor);
        drop(read_txn);
        
        if (db_idx + 1) % 5 == 0 {
            let current_rss = get_rss_kb();
            let overhead = current_rss - baseline_rss;
            let elapsed = start.elapsed().as_secs();
            println!(
                "  After {} databases ({}K docs): RSS = {} MB (+{} MB), Time: {}s, Read: {} MB",
                db_idx + 1,
                (db_idx + 1) * docs_per_db / 1000,
                current_rss / 1024,
                overhead / 1024,
                elapsed,
                total_bytes / (1024 * 1024)
            );
        }
    }
    
    let final_rss = get_rss_kb();
    let total_overhead = final_rss - baseline_rss;
    let total_time = start.elapsed();
    
    println!(
        "\nFinal: {} databases, {}K total docs",
        num_dbs,
        num_dbs * docs_per_db / 1000
    );
    println!(
        "Total RSS: {} MB, Overhead: {} MB (~{} MB/db)",
        final_rss / 1024,
        total_overhead / 1024,
        total_overhead / 1024 / num_dbs
    );
    println!("Total time: {:.1}s, {:.1}s per database", total_time.as_secs_f64(), total_time.as_secs_f64() / num_dbs as f64);
    
    Ok(())
}

/// Test 3: Query performance across multiple databases
fn test_query_performance(num_dbs: usize, docs_per_db: usize) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 3: Query Performance ===");
    println!("Testing query latency across {} databases...", num_dbs);
    
    let base_dir = PathBuf::from("/tmp/lmdb_test_query");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir)?;
    
    let env = Environment::new()
        .set_max_dbs(num_dbs as u32)
        .set_map_size(50 * 1024 * 1024 * 1024)
        .open(&base_dir)?;
    
    let mut rng = rand::thread_rng();
    let mut databases = Vec::new();
    
    // Create and populate databases
    println!("Setting up databases...");
    for db_idx in 0..num_dbs {
        let db_name = format!("tenant_{}", db_idx);
        let db = env.create_db(Some(&db_name), lmdb::DatabaseFlags::empty())?;
        
        let mut inverted_index: HashMap<String, Vec<u32>> = HashMap::new();
        
        for doc_id in 0..docs_per_db {
            let title = generate_product_title(&mut rng);
            let body = generate_product_description(&mut rng);
            let text = format!("{} {}", title, body);
            
            for token in text.split_whitespace() {
                let term = token.to_lowercase();
                inverted_index.entry(term).or_insert_with(Vec::new).push(doc_id as u32);
            }
        }
        
        let mut txn = env.begin_rw_txn()?;
        for (term, postings) in inverted_index.iter() {
            let encoded = encode_posting_list(postings);
            txn.put(db, &term, &encoded, WriteFlags::empty())?;
        }
        txn.commit()?;
        
        databases.push(db);
    }
    
    println!("Running queries...");
    
    let queries = vec!["laptop", "phone", "wireless", "gaming"];
    
    for query_term in &queries {
        let mut total_time = 0u128;
        let mut total_results = 0;
        
        for db in &databases {
            let txn = env.begin_ro_txn()?;
            
            let start = Instant::now();
            if let Ok(encoded) = txn.get(*db, &query_term) {
                let postings = decode_posting_list(encoded);
                total_results += postings.len();
            }
            let elapsed = start.elapsed().as_micros();
            
            total_time += elapsed;
        }
        
        let avg_latency = total_time / num_dbs as u128;
        println!(
            "  Query '{}': avg {:.2}μs ({:.3}ms), total {} results across {} databases",
            query_term,
            avg_latency,
            avg_latency as f64 / 1000.0,
            total_results,
            num_dbs
        );
    }
    
    Ok(())
}

/// Test 4: Columnar data (DocValues) performance
fn test_columnar_storage(num_docs: usize) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 4: Columnar Storage Performance ===");
    println!("Testing columnar data access with {} docs...", num_docs);
    
    let base_dir = PathBuf::from("/tmp/lmdb_test_columnar");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir)?;
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(&base_dir)?;
    
    // Separate databases for columnar data (similar to DocValues)
    let price_db = env.create_db(Some("price_column"), lmdb::DatabaseFlags::INTEGER_KEY)?;
    let category_db = env.create_db(Some("category_column"), lmdb::DatabaseFlags::INTEGER_KEY)?;
    
    let mut rng = rand::thread_rng();
    
    println!("Writing columnar data...");
    let start = Instant::now();
    let mut txn = env.begin_rw_txn()?;
    
    for doc_id in 0..num_docs {
        let price = rng.gen_range(10..1000u64);
        let category_id = rng.gen_range(0..6u64);
        
        txn.put(price_db, &doc_id.to_ne_bytes(), &price.to_ne_bytes(), WriteFlags::empty())?;
        txn.put(category_db, &doc_id.to_ne_bytes(), &category_id.to_ne_bytes(), WriteFlags::empty())?;
    }
    
    txn.commit()?;
    println!("  Wrote {} docs in {:.1}s", num_docs, start.elapsed().as_secs_f64());
    
    // Test random access performance (query-time sorting scenario)
    println!("Testing random access (query-time sort simulation)...");
    
    let doc_ids: Vec<usize> = (0..100).map(|_| rng.gen_range(0..num_docs)).collect();
    
    let txn = env.begin_ro_txn()?;
    let start = Instant::now();
    
    let mut prices = Vec::new();
    for doc_id in &doc_ids {
        if let Ok(price_bytes) = txn.get(price_db, &doc_id.to_ne_bytes()) {
            let price = u64::from_ne_bytes(price_bytes.try_into().unwrap());
            prices.push(price);
        }
    }
    
    let access_time = start.elapsed();
    println!(
        "  Random access 100 doc prices: {:.2}μs total, {:.2}μs per access",
        access_time.as_micros(),
        access_time.as_micros() as f64 / 100.0
    );
    
    // Simulate sort
    prices.sort();
    println!("  (Simulated sort of 100 prices)");
    
    Ok(())
}

/// Test 5: Write transaction performance (hot index flush simulation)
fn test_write_performance() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 5: Write Transaction Performance ===");
    println!("Simulating hot index flush pattern...");
    
    let base_dir = PathBuf::from("/tmp/lmdb_test_writes");
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&base_dir)?;
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(10 * 1024 * 1024 * 1024)
        .open(&base_dir)?;
    
    let db = env.create_db(Some("test"), lmdb::DatabaseFlags::empty())?;
    
    let mut rng = rand::thread_rng();
    
    // Simulate 10 flushes
    for flush_num in 1..=10 {
        println!("Flush #{}: Writing ~5K term entries...", flush_num);
        
        let start = Instant::now();
        let mut txn = env.begin_rw_txn()?;
        
        // Simulate adding terms from ~5K docs
        for i in 0..5000 {
            let term = format!("term_{}_{}_{}", flush_num, i, rng.gen_range(0..1000));
            let postings = encode_posting_list(&vec![i as u32, (i+1) as u32, (i+2) as u32]);
            txn.put(db, &term, &postings, WriteFlags::empty())?;
        }
        
        let write_time = start.elapsed();
        let commit_start = Instant::now();
        txn.commit()?;
        let commit_time = commit_start.elapsed();
        
        let total_time = start.elapsed();
        println!(
            "  Write: {:.2}ms, Commit: {:.2}ms, Total: {:.2}ms",
            write_time.as_millis(),
            commit_time.as_millis(),
            total_time.as_millis()
        );
    }
    
    Ok(())
}

/// Test 6: Database migration simulation (tenant movement)
fn test_migration_pattern() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== TEST 6: Migration Pattern (Tenant Movement) ===");
    
    let src_dir = PathBuf::from("/tmp/lmdb_test_migrate_src");
    let dst_dir = PathBuf::from("/tmp/lmdb_test_migrate_dst");
    
    let _ = fs::remove_dir_all(&src_dir);
    let _ = fs::remove_dir_all(&dst_dir);
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&dst_dir)?;
    
    // Create source environment with tenant data
    println!("Setting up source tenant...");
    let src_env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(1024 * 1024 * 1024)
        .open(&src_dir)?;
    
    let tenant_db = src_env.create_db(Some("tenant_123"), lmdb::DatabaseFlags::empty())?;
    
    let mut txn = src_env.begin_rw_txn()?;
    for i in 0..1000 {
        let key = format!("term_{}", i);
        let value = format!("postings_{}", i);
        txn.put(tenant_db, &key, &value, WriteFlags::empty())?;
    }
    txn.commit()?;
    
    // Measure migration time
    println!("Migrating tenant database...");
    let start = Instant::now();
    
    // LMDB migration: copy environment files
    // In production, you'd use proper backup/restore or mdb_copy
    let copy_result = std::process::Command::new("cp")
        .arg("-r")
        .arg(&src_dir)
        .arg(&dst_dir)
        .status();
    
    let migration_time = start.elapsed();
    
    if copy_result.is_ok() {
        println!("  Migration completed in {:.2}s", migration_time.as_secs_f64());
        
        // Verify destination
        let dst_env = Environment::new()
            .set_max_dbs(10)
            .open(&dst_dir.join("lmdb_test_migrate_src"))?;
        
        let dst_db = dst_env.open_db(Some("tenant_123"))?;
        let txn = dst_env.begin_ro_txn()?;
        let mut cursor = txn.open_ro_cursor(dst_db)?;
        let count = cursor.iter().count();
        
        println!("  Verified: {} entries in migrated database", count);
    } else {
        println!("  Migration simulation failed (requires 'cp' command)");
    }
    
    Ok(())
}

// Helper functions

fn encode_posting_list(postings: &[u32]) -> Vec<u8> {
    // Simple delta encoding
    let mut encoded = Vec::new();
    let mut prev = 0u32;
    
    for &doc_id in postings {
        let delta = doc_id - prev;
        encoded.extend_from_slice(&delta.to_ne_bytes());
        prev = doc_id;
    }
    
    encoded
}

fn decode_posting_list(encoded: &[u8]) -> Vec<u32> {
    let mut postings = Vec::new();
    let mut current = 0u32;
    
    for chunk in encoded.chunks(4) {
        if chunk.len() == 4 {
            let delta = u32::from_ne_bytes(chunk.try_into().unwrap());
            current += delta;
            postings.push(current);
        }
    }
    
    postings
}

fn generate_product_title(rng: &mut rand::rngs::ThreadRng) -> String {
    let adjectives = ["Professional", "Premium", "Budget", "Wireless", "Gaming", "Portable"];
    let products = ["Laptop", "Phone", "Headphones", "Monitor", "Keyboard", "Mouse", "Camera"];
    
    format!(
        "{} {} {}",
        adjectives[rng.gen_range(0..adjectives.len())],
        products[rng.gen_range(0..products.len())],
        rng.gen_range(1000..9999)
    )
}

fn generate_product_description(rng: &mut rand::rngs::ThreadRng) -> String {
    let features = [
        "High performance processor with advanced cooling",
        "Ultra HD display with vibrant colors",
        "Long battery life up to 12 hours",
        "Premium build quality with aluminum chassis",
        "Fast charging support included",
        "Wireless connectivity with Bluetooth 5.0",
    ];
    
    let mut desc = String::new();
    for _ in 0..rng.gen_range(2..5) {
        desc.push_str(features[rng.gen_range(0..features.len())]);
        desc.push_str(". ");
    }
    desc
}

fn get_rss_kb() -> usize {
    #[cfg(target_os = "linux")]
    {
        let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return parts[1].parse::<usize>().unwrap_or(0);
                }
            }
        }
        return 0;
    }
    
    #[cfg(target_os = "macos")]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe {
            libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        }
        // ru_maxrss on macOS is in bytes, convert to KB
        return (usage.ru_maxrss / 1024) as usize;
    }
    
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        println!("WARNING: RSS measurement only works on Linux and macOS");
        return 0;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================================");
    println!("LMDB MULTI-TENANT OVERHEAD TEST");
    println!("========================================");
    println!("Testing custom inverted index on LMDB");
    println!("This will take 5-10 minutes...\n");
    
    // Test 1: Empty databases overhead
    test_empty_db_overhead(100)?;
    
    // Test 2: Realistic inverted index
    test_realistic_inverted_index(20, 10_000)?;
    
    // Test 3: Query performance
    test_query_performance(20, 10_000)?;
    
    // Test 4: Columnar storage (DocValues equivalent)
    test_columnar_storage(100_000)?;
    
    // Test 5: Write performance
    test_write_performance()?;
    
    // Test 6: Migration pattern
    test_migration_pattern()?;
    
    println!("\n========================================");
    println!("TEST SUITE COMPLETE");
    println!("========================================");
    println!("\nKEY METRICS TO EVALUATE:");
    println!("1. Memory overhead per database (target: <50MB acceptable, <20MB ideal)");
    println!("2. Query latency (target: <1ms for term lookup, <50ms for complex queries)");
    println!("3. Write transaction time (target: <500ms for 5K term writes)");
    println!("4. Random access time for columnar data (target: <10μs per access)");
    
    println!("\nCOMPARE WITH TANTIVY:");
    println!("- LMDB overhead should be lower (no FST overhead per database)");
    println!("- Query latency may be higher (no optimized posting list structures)");
    println!("- More control over data structures");
    println!("- More implementation work required (FST, BM25, compression)");
    
    println!("\nDECISION CRITERIA:");
    println!("✓ CHOOSE LMDB if:");
    println!("  - Need <50MB overhead per tenant");
    println!("  - Willing to build custom inverted index (6-8 weeks)");
    println!("  - Need precise control over data layout");
    println!("\n✓ CHOOSE TANTIVY if:");
    println!("  - Overhead <100MB acceptable");
    println!("  - Want to ship faster (2 weeks vs 8 weeks)");
    println!("  - Trust battle-tested search library");
    
    Ok(())
}