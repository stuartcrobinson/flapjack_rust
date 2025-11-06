// Test: LMDB Document Replication CPU Cost
//
// Question: How expensive is re-indexing on replicas vs copying segments?
//
// Context: Discussion claims "30 replicas = 31x CPU" but never measured.
// This test simulates:
// 1. Primary: Index documents, measure CPU time
// 2. Replicas: Re-index same documents, measure cumulative CPU
// 3. Compare to theoretical segment copy (network only, 0 CPU)

use heed::{EnvOpenOptions, Database};
use heed::types::*;
use std::time::Instant;
use std::path::Path;
use tempfile::TempDir;

const BATCH_SIZE: usize = 100;
const BATCHES: usize = 10; // 1K total docs
const REPLICA_COUNT: usize = 10; // Test 10 replicas (scale to 30 extrapolates)

type BM25Index = Database<U32<byteorder::NativeEndian>, Str>;

fn create_index(path: &Path) -> (heed::Env, BM25Index) {
    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(100 * 1024 * 1024) // 100MB
            .max_dbs(10)
            .open(path)
            .unwrap()
    };
    
    let mut wtxn = env.write_txn().unwrap();
    let db = env.create_database(&mut wtxn, Some("posting_lists")).unwrap();
    wtxn.commit().unwrap();
    
    (env, db)
}

fn index_documents(env: &heed::Env, db: BM25Index, start_id: u32) -> std::time::Duration {
    let start = Instant::now();
    
    for batch in 0..BATCHES {
        let mut wtxn = env.write_txn().unwrap();
        
        for i in 0..BATCH_SIZE {
            let doc_id = start_id + (batch * BATCH_SIZE + i) as u32;
            
            // Simulate BM25 indexing: 6 B-tree writes per doc
            // (posting lists for ~5 terms + doc metadata)
            for term_id in 0..6 {
                let key = doc_id * 1000 + term_id;
                let value = format!("doc_{}_term_{}_data", doc_id, term_id);
                db.put(&mut wtxn, &key, &value).unwrap();
            }
        }
        
        wtxn.commit().unwrap();
    }
    
    start.elapsed()
}

fn main() {
    println!("\n=== LMDB Replication CPU Cost Test ===\n");
    
    // Step 1: Primary indexing
    println!("Step 1: Primary indexing {} documents...", BATCH_SIZE * BATCHES);
    
    let primary_dir = TempDir::new().unwrap();
    let (primary_env, primary_db) = create_index(primary_dir.path());
    
    let primary_time = index_documents(&primary_env, primary_db, 0);
    let primary_ms = primary_time.as_secs_f64() * 1000.0;
    
    println!("  Primary indexing: {:.2}ms", primary_ms);
    println!("  Per document: {:.3}ms", primary_ms / (BATCH_SIZE * BATCHES) as f64);
    
    let primary_size_bytes = std::fs::metadata(primary_dir.path().join("data.mdb"))
        .map(|m| m.len())
        .unwrap_or(0);
    let primary_size_mb = primary_size_bytes as f64 / (1024.0 * 1024.0);
    println!("  Primary index size: {:.2} MB", primary_size_mb);
    
    // Step 2: Replica re-indexing (simulate document replication)
    println!("\nStep 2: Simulating {} replicas (document replication)...", REPLICA_COUNT);
    
    let mut replica_times = Vec::new();
    let mut replica_dirs = Vec::new();
    
    for i in 0..REPLICA_COUNT {
        let replica_dir = TempDir::new().unwrap();
        let (replica_env, replica_db) = create_index(replica_dir.path());
        
        // Re-index same documents
        let replica_time = index_documents(&replica_env, replica_db, 0);
        replica_times.push(replica_time);
        replica_dirs.push((replica_env, replica_db, replica_dir));
        
        if (i + 1) % 5 == 0 {
            println!("  Indexed replica {}/{}", i + 1, REPLICA_COUNT);
        }
    }
    
    let total_replica_ms: f64 = replica_times.iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .sum();
    let avg_replica_ms = total_replica_ms / REPLICA_COUNT as f64;
    
    println!("  Avg replica indexing: {:.2}ms", avg_replica_ms);
    println!("  Total replica CPU: {:.2}ms", total_replica_ms);
    
    // Step 3: Theoretical segment replication cost
    println!("\nStep 3: Theoretical segment replication cost...");
    
    // Network transfer cost (ignore CPU, assume file copy)
    // Estimate: 100 MB/s network = 1ms per 100KB
    let transfer_ms = (primary_size_mb * 1024.0) / 100.0; // 100MB/s
    let total_transfer_ms = transfer_ms * REPLICA_COUNT as f64;
    
    println!("  Network transfer/replica: {:.2}ms (@ 100MB/s)", transfer_ms);
    println!("  Total transfer cost (all replicas): {:.2}ms", total_transfer_ms);
    
    // Step 4: Comparison
    println!("\n=== COST COMPARISON ===");
    println!("\nDocument Replication (LMDB):");
    println!("  Primary: {:.2}ms", primary_ms);
    println!("  Replicas: {:.2}ms (total CPU)", total_replica_ms);
    println!("  Total: {:.2}ms", primary_ms + total_replica_ms);
    println!("  CPU amplification: {:.1}x", (primary_ms + total_replica_ms) / primary_ms);
    
    println!("\nSegment Replication (Tantivy-style):");
    println!("  Primary: {:.2}ms (indexing)", primary_ms);
    println!("  Replicas: {:.2}ms (network only, ~0 CPU)", total_transfer_ms);
    println!("  Total: {:.2}ms", primary_ms + total_transfer_ms);
    println!("  CPU amplification: {:.2}x (trivial)", total_transfer_ms / primary_ms);
    
    println!("\nReplication cost difference:");
    let cpu_saved = total_replica_ms - total_transfer_ms;
    println!("  CPU saved with segment replication: {:.2}ms", cpu_saved);
    println!("  Speedup: {:.1}x", total_replica_ms / total_transfer_ms);
    
    // Step 5: Extrapolate to 30 replicas
    println!("\n=== EXTRAPOLATION TO 30 REPLICAS ===");
    
    let scale_factor = 30.0 / REPLICA_COUNT as f64;
    let doc_repl_30 = (primary_ms + total_replica_ms) * scale_factor;
    let seg_repl_30 = primary_ms + (total_transfer_ms * scale_factor);
    
    println!("\nAt 30 replicas (1,000 docs/sec write rate):");
    println!("  Document replication: {:.0}ms CPU/sec = {:.1} cores", 
        doc_repl_30, doc_repl_30 / 1000.0);
    println!("  Segment replication: {:.0}ms CPU/sec = {:.2} cores",
        seg_repl_30, seg_repl_30 / 1000.0);
    
    let cores_saved = (doc_repl_30 - seg_repl_30) / 1000.0;
    println!("\n  Cores saved: {:.1}", cores_saved);
    println!("  Cost saved @ $0.03/core-hour: ${:.2}/hour = ${:.0}/month",
        cores_saved * 0.03, cores_saved * 0.03 * 730.0);
    
    // Decision matrix
    println!("\n=== DECISION MATRIX ===");
    
    if cores_saved < 1.0 {
        println!("✅ Document replication acceptable: <1 core difference");
        println!("   LMDB viable even with 30 replicas");
    } else if cores_saved < 5.0 {
        println!("⚠️  Marginal: {:.1} cores saved with segment replication", cores_saved);
        println!("   Cost: ${:.0}/month", cores_saved * 0.03 * 730.0);
        println!("   Recommendation: Use segment replication if >50% customers want >10 replicas");
    } else {
        println!("❌ CRITICAL: {:.1} cores wasted on replication", cores_saved);
        println!("   Cost: ${:.0}/month", cores_saved * 0.03 * 730.0);
        println!("   Recommendation: Segment replication REQUIRED for multi-region");
    }
    
    println!("\nKey insight:");
    if total_replica_ms / total_transfer_ms > 10.0 {
        println!("  Document re-indexing is {:.0}x more expensive than file transfer", 
            total_replica_ms / total_transfer_ms);
        println!("  At scale, this compounds to unsustainable CPU waste");
    } else {
        println!("  Re-indexing overhead minimal - network dominates replication cost");
    }
}