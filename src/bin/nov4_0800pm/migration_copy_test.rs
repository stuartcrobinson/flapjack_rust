// Test: Tenant migration mechanics with live writes
// Question: Can we copy LMDB file while serving reads/writes? How long?
//
// Critical for "dynamic juggling" requirement: move tenants between machines.
// Need to validate:
// 1. Copy duration for realistic tenant sizes
// 2. File consistency during copy (LMDB MVCC should handle this)
// 3. Whether we need write pause or can hot-copy
//
// Method:
// 1. Create tenant with ~100K docs (~100-500 MB)
// 2. Background thread: continuous writes at 10/sec
// 3. Foreground: copy entire LMDB directory
// 4. Verify copied file opens and has correct doc count
//
// Success: Copy completes in <10 sec, copied DB is consistent
// Failure: Copy takes >60 sec OR copied file is corrupted

use lmdb::{Environment, Transaction, WriteFlags, Cursor};
use std::fs;
use std::time::{Duration, Instant};
use std::path::{Path, PathBuf};
use std::thread;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

fn get_dir_size_mb(path: &Path) -> f64 {
    let output = std::process::Command::new("du")
        .args(&["-sm", path.to_str().unwrap()])
        .output()
        .unwrap();
    let size_str = String::from_utf8_lossy(&output.stdout);
    size_str.split_whitespace()
        .next()
        .unwrap()
        .parse::<f64>()
        .unwrap_or(0.0)
}

fn create_large_tenant(path: &Path, doc_count: usize) {
    fs::create_dir_all(path).unwrap();
    
    let env = Environment::new()
        .set_max_dbs(10)
        .set_map_size(1_000_000_000) // 1 GB
        .open(path)
        .unwrap();
    
    let db = env.create_db(Some("docs"), lmdb::DatabaseFlags::empty()).unwrap();
    
    println!("Writing {} docs...", doc_count);
    let batch_size = 1000;
    
    for batch_start in (0..doc_count).step_by(batch_size) {
        let mut txn = env.begin_rw_txn().unwrap();
        let batch_end = (batch_start + batch_size).min(doc_count);
        
        for i in batch_start..batch_end {
            let key = format!("doc_{:08}", i);
            // ~500 bytes per doc
            let value = format!(
                "{{\"id\":{},\"title\":\"Document {}\",\"body\":\"{}\"}}",
                i, i, 
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
                Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris. \
                Nisi ut aliquip ex ea commodo consequat duis aute irure dolor in. \
                Reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla. \
                Excepteur sint occaecat cupidatat non proident sunt in culpa qui."
            );
            txn.put(db, &key, &value, WriteFlags::empty()).unwrap();
        }
        
        txn.commit().unwrap();
        
        if batch_start % 10000 == 0 {
            print!(".");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
        }
    }
    
    println!("\nDocs written.");
}

fn background_writer(
    path: PathBuf, 
    should_stop: Arc<AtomicBool>,
    write_count: Arc<AtomicU64>
) {
    let env = Environment::new()
        .set_max_dbs(10)
        .open(&path)
        .unwrap();
    let db = env.open_db(Some("docs")).unwrap(); // DB already created by create_large_tenant
    
    let mut counter = 0u64;
    while !should_stop.load(Ordering::Relaxed) {
        let mut txn = env.begin_rw_txn().unwrap();
        
        for _ in 0..10 {
            let key = format!("live_write_{:08}", counter);
            let value = format!("{{\"counter\":{},\"ts\":{}}}", 
                counter, 
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            );
            txn.put(db, &key, &value, WriteFlags::empty()).unwrap();
            counter += 1;
        }
        
        txn.commit().unwrap();
        write_count.fetch_add(10, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(100)); // 10 writes/sec
    }
}

fn copy_directory(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        
        if file_type.is_dir() {
            copy_directory(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    
    Ok(())
}

fn count_docs(path: &Path, db_name: &str) -> usize {
    let env = Environment::new()
        .set_max_dbs(10)
        .set_flags(lmdb::EnvironmentFlags::READ_ONLY)
        .open(path)
        .unwrap();
    let db = env.open_db(Some(db_name)).unwrap();
    let txn = env.begin_ro_txn().unwrap();
    
    let mut cursor = txn.open_ro_cursor(db).unwrap();
    let mut count = 0;
    
    for _ in cursor.iter() {
        count += 1;
    }
    
    count
}

fn main() {
    let base_path = "/tmp/flapjack_migration_test";
    let source_path = PathBuf::from(format!("{}/source_tenant", base_path));
    let dest_path = PathBuf::from(format!("{}/migrated_tenant", base_path));
    
    // Cleanup
    let _ = fs::remove_dir_all(base_path);
    fs::create_dir_all(base_path).unwrap();
    
    println!("=== Tenant Migration Test ===\n");
    
    // Phase 1: Create realistic-sized tenant
    println!("--- Phase 1: Create tenant with 100K docs ---");
    let doc_count = 100_000;
    create_large_tenant(&source_path, doc_count);
    
    let size_mb = get_dir_size_mb(&source_path);
    println!("Tenant size: {:.1} MB", size_mb);
    
    let initial_doc_count = count_docs(&source_path, "docs");
    println!("Initial doc count: {}", initial_doc_count);
    
    // Phase 2: Start background writes
    println!("\n--- Phase 2: Start background writes (10/sec) ---");
    let should_stop = Arc::new(AtomicBool::new(false));
    let write_count = Arc::new(AtomicU64::new(0));
    
    let writer_handle = {
        let path = source_path.clone();
        let should_stop = Arc::clone(&should_stop);
        let write_count = Arc::clone(&write_count);
        thread::spawn(move || background_writer(path, should_stop, write_count))
    };
    
    thread::sleep(Duration::from_millis(500)); // Let writer start
    println!("Background writer active");
    
    // Phase 3: Hot copy while writes continue
    println!("\n--- Phase 3: Hot copy tenant directory ---");
    println!("Copying {} MB with active writes...", size_mb);
    
    let copy_start = Instant::now();
    copy_directory(&source_path, &dest_path).unwrap();
    let copy_duration = copy_start.elapsed();
    
    let copy_ms = copy_duration.as_millis();
    let copy_sec = copy_duration.as_secs_f64();
    let throughput_mbps = size_mb / copy_sec;
    
    println!("\nCopy completed:");
    println!("  Duration: {:.2} seconds ({} ms)", copy_sec, copy_ms);
    println!("  Throughput: {:.1} MB/s", throughput_mbps);
    
    if copy_sec < 5.0 {
        println!("  ✅ EXCELLENT: Copy very fast");
    } else if copy_sec < 15.0 {
        println!("  ✅ ACCEPTABLE: Copy reasonably fast");
    } else if copy_sec < 60.0 {
        println!("  ⚠️  SLOW: May impact migration latency");
    } else {
        println!("  ❌ VERY SLOW: Unacceptable for live migration");
    }
    
    // Stop background writer
    should_stop.store(true, Ordering::Relaxed);
    writer_handle.join().unwrap();
    
    let total_writes = write_count.load(Ordering::Relaxed);
    println!("\nBackground writes during copy: {}", total_writes);
    
    // Phase 4: Verify copied database
    println!("\n--- Phase 4: Verify copied database ---");
    
    let copied_doc_count = count_docs(&dest_path, "docs");
    println!("Copied DB doc count: {}", copied_doc_count);
    
    // The copied DB should have initial docs (may not have live writes that happened during copy)
    // This is expected - MVCC means copy sees a snapshot
    if copied_doc_count >= initial_doc_count {
        println!("✅ Copied DB is consistent (has {} >= {} initial docs)", 
            copied_doc_count, initial_doc_count);
    } else {
        println!("❌ Copied DB is INCONSISTENT ({} < {} initial docs)", 
            copied_doc_count, initial_doc_count);
    }
    
    // Test that copied DB is readable
    println!("\nTesting copied DB is functional...");
    let copied_env = Environment::new()
        .set_max_dbs(10)
        .open(&dest_path)
        .unwrap();
    let copied_db = copied_env.open_db(Some("docs")).unwrap();
    let copied_txn = copied_env.begin_ro_txn().unwrap();
    
    // Read some random docs
    let mut read_success = 0;
    for i in [0, 1000, 50000, 99999] {
        let key = format!("doc_{:08}", i);
        if copied_txn.get(copied_db, &key).is_ok() {
            read_success += 1;
        }
    }
    
    println!("Random doc reads: {}/4 successful", read_success);
    
    if read_success == 4 {
        println!("✅ Copied DB is fully functional");
    } else {
        println!("❌ Copied DB has read errors");
    }
    
    // Phase 5: Test compacted copy
    println!("\n--- Phase 5: Test mdb_copy with compaction ---");
    let mdb_copy_path = PathBuf::from(format!("{}/mdb_copy_tenant", base_path));
    fs::create_dir_all(&mdb_copy_path).unwrap();
    
    let compact_start = Instant::now();
    let output = std::process::Command::new("mdb_copy")
        .args(&[
            "-c", // compact
            source_path.to_str().unwrap(),
            mdb_copy_path.to_str().unwrap()
        ])
        .output();
    
    match output {
        Ok(result) => {
            let compact_duration = compact_start.elapsed();
            
            if result.status.success() {
                let mdb_size = get_dir_size_mb(&mdb_copy_path);
                let original_size = get_dir_size_mb(&source_path);
                let size_reduction = ((original_size - mdb_size) / original_size) * 100.0;
                
                println!("mdb_copy completed in {:.2} sec", compact_duration.as_secs_f64());
                println!("Original size: {:.1} MB", original_size);
                println!("Compacted size: {:.1} MB ({:.1}% reduction)", mdb_size, size_reduction);
                
                let mdb_doc_count = count_docs(&mdb_copy_path, "docs");
                println!("mdb_copy doc count: {}", mdb_doc_count);
                
                if mdb_doc_count >= initial_doc_count {
                    println!("✅ mdb_copy produced consistent database");
                } else {
                    println!("❌ mdb_copy database is inconsistent");
                }
            } else {
                println!("⚠️  mdb_copy failed (may not be installed)");
                println!("stderr: {}", String::from_utf8_lossy(&result.stderr));
            }
        }
        Err(e) => {
            println!("⚠️  mdb_copy not available: {}", e);
            println!("Install with: apt-get install lmdb-utils");
        }
    }
    
    // Summary
    println!("\n=== SUMMARY ===");
    println!("Tenant size: {:.1} MB", size_mb);
    println!("Copy method: filesystem cp");
    println!("  Duration: {:.2} sec", copy_sec);
    println!("  Writes during copy: {}", total_writes);
    println!("  Consistency: {}", if copied_doc_count >= initial_doc_count { "✅ OK" } else { "❌ FAIL" });
    println!("\nMigration strategy viable: {}", 
        if copy_sec < 30.0 && copied_doc_count >= initial_doc_count {
            "✅ YES - Hot copy works"
        } else {
            "❌ NO - Need write pause or mdb_copy"
        });
    
    // Cleanup
    println!("\nCleaning up...");
    let _ = fs::remove_dir_all(base_path);
}