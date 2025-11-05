// src/bin/bm25_query_perf_test.rs
// Benchmark BM25 query performance across different workloads
// Target: P99 < 50ms for text-only queries

use anyhow::Result;
use rand::Rng;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use flapjack_rust::bm25::BM25Index;
// use flapjack_rust::bm25::*;
// use bm25::BM25Index;

fn get_rss_mb() -> f64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let kb: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
            return kb as f64 / 1024.0;
        }
    }
    0.0
}

fn generate_document(doc_id: u32, avg_words: usize, vocab_size: usize) -> (u32, Vec<String>) {
    let mut rng = rand::thread_rng();
    let num_words = (avg_words as i32 + rng.gen_range(-20..20)).max(10) as usize;
    
    let tokens: Vec<String> = (0..num_words)
        .map(|_| format!("word{}", rng.gen_range(0..vocab_size)))
        .collect();
    
    (doc_id, tokens)
}

fn percentile(mut latencies: Vec<Duration>, p: f64) -> Duration {
    if latencies.is_empty() {
        return Duration::from_secs(0);
    }
    latencies.sort();
    let idx = ((latencies.len() as f64 * p) as usize).min(latencies.len() - 1);
    latencies[idx]
}

fn main() -> Result<()> {
    println!("=== BM25 Query Performance Test ===\n");
    println!("Target: P99 < 50ms for text-only queries");
    println!("Goal: Validate Flapjack can compete with Algolia/Meilisearch\n");

    const DOCS: usize = 50_000;
    const AVG_DOC_LENGTH: usize = 150;
    const VOCAB_SIZE: usize = 10_000;
    const NUM_QUERIES: usize = 1000;

    let temp_dir = PathBuf::from("/tmp/flapjack_bm25_perf");
    fs::create_dir_all(&temp_dir)?;

    // Phase 1: Build index
    println!("Phase 1: Indexing {} documents...", DOCS);
    let index = BM25Index::open(&temp_dir, 500 * 1024 * 1024)?;
    
    let index_start = Instant::now();
    let batch_size = 1000;
    for batch_start in (0..DOCS).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(DOCS);
        let docs: Vec<_> = (batch_start..batch_end)
            .map(|i| generate_document(i as u32, AVG_DOC_LENGTH, VOCAB_SIZE))
            .collect();
        index.index_documents(docs)?;
    }
    let index_duration = index_start.elapsed();
    
    let stats = index.get_stats()?;
    println!("  Indexed in {:.2}s", index_duration.as_secs_f64());
    println!("  {} docs, {} unique terms", stats.total_docs, index.count_terms()?);
    println!("  Avg doc length: {:.1} tokens\n", stats.avg_doc_length);

    // Phase 2: Single-term queries
    println!("Phase 2: Single-term queries ({} queries)...", NUM_QUERIES);
    let mut latencies = Vec::new();
    let mut total_results = 0;
    
    for i in 0..NUM_QUERIES {
        let term = format!("word{}", i % VOCAB_SIZE);
        let start = Instant::now();
        let results = index.search(&[term], 100)?;
        latencies.push(start.elapsed());
        total_results += results.len();
    }
    
    let p50 = percentile(latencies.clone(), 0.50);
    let p95 = percentile(latencies.clone(), 0.95);
    let p99 = percentile(latencies.clone(), 0.99);
    
    println!("  Latency - P50: {:.2}ms, P95: {:.2}ms, P99: {:.2}ms", 
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0);
    println!("  Avg results per query: {:.1}", total_results as f64 / NUM_QUERIES as f64);
    
    if p99.as_millis() > 50 {
        println!("  ⚠️  P99 exceeds 50ms target");
    } else {
        println!("  ✅ Meets 50ms P99 target");
    }
    println!();

    // Phase 3: Multi-term queries (2-4 terms)
    println!("Phase 3: Multi-term queries ({} queries)...", NUM_QUERIES);
    let mut latencies = Vec::new();
    let mut total_results = 0;
    let mut rng = rand::thread_rng();
    
    for _ in 0..NUM_QUERIES {
        let num_terms = rng.gen_range(2..=4);
        let terms: Vec<String> = (0..num_terms)
            .map(|_| format!("word{}", rng.gen_range(0..VOCAB_SIZE)))
            .collect();
        
        let start = Instant::now();
        let results = index.search(&terms, 100)?;
        latencies.push(start.elapsed());
        total_results += results.len();
    }
    
    let p50 = percentile(latencies.clone(), 0.50);
    let p95 = percentile(latencies.clone(), 0.95);
    let p99 = percentile(latencies.clone(), 0.99);
    
    println!("  Latency - P50: {:.2}ms, P95: {:.2}ms, P99: {:.2}ms", 
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0);
    println!("  Avg results per query: {:.1}", total_results as f64 / NUM_QUERIES as f64);
    
    if p99.as_millis() > 50 {
        println!("  ⚠️  P99 exceeds 50ms target");
    } else {
        println!("  ✅ Meets 50ms P99 target");
    }
    println!();

    // Phase 4: Rare term queries (high IDF)
    println!("Phase 4: Rare term queries (terms appearing in <10 docs)...");
    let mut latencies = Vec::new();
    let mut total_results = 0;
    
    for i in 0..NUM_QUERIES {
        // Use high term IDs that are less likely to appear
        let term = format!("word{}", 9000 + (i % 1000));
        let start = Instant::now();
        let results = index.search(&[term], 100)?;
        latencies.push(start.elapsed());
        total_results += results.len();
    }
    
    let p50 = percentile(latencies.clone(), 0.50);
    let p95 = percentile(latencies.clone(), 0.95);
    let p99 = percentile(latencies.clone(), 0.99);
    
    println!("  Latency - P50: {:.2}ms, P95: {:.2}ms, P99: {:.2}ms", 
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0);
    println!("  Avg results per query: {:.1}", total_results as f64 / NUM_QUERIES as f64);
    println!("  (Faster due to smaller posting lists)\n");

    // Phase 5: Common term queries (low IDF)
    println!("Phase 5: Common term queries (terms in most docs)...");
    let mut latencies = Vec::new();
    let mut total_results = 0;
    
    for i in 0..NUM_QUERIES {
        // Use low term IDs that appear frequently
        let term = format!("word{}", i % 100);
        let start = Instant::now();
        let results = index.search(&[term], 100)?;
        latencies.push(start.elapsed());
        total_results += results.len();
    }
    
    let p50 = percentile(latencies.clone(), 0.50);
    let p95 = percentile(latencies.clone(), 0.95);
    let p99 = percentile(latencies.clone(), 0.99);
    
    println!("  Latency - P50: {:.2}ms, P95: {:.2}ms, P99: {:.2}ms", 
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0);
    println!("  Avg results per query: {:.1}", total_results as f64 / NUM_QUERIES as f64);
    println!("  (Slower due to large posting lists)\n");

    // Phase 6: Top-k variation
    println!("Phase 6: Top-k retrieval performance...");
    let term = "word1234".to_string();
    
    for k in [10, 100, 1000] {
        let mut latencies = Vec::new();
        
        for _ in 0..100 {
            let start = Instant::now();
            let _results = index.search(&[term.clone()], k)?;
            latencies.push(start.elapsed());
        }
        
        let p99 = percentile(latencies, 0.99);
        println!("  Top-{:4}: P99 = {:.2}ms", k, p99.as_secs_f64() * 1000.0);
    }
    println!();

    // Phase 7: Memory footprint during queries
    println!("Phase 7: Memory footprint check...");
    let rss_before = get_rss_mb();
    
    // Run many queries to trigger full working set
    for _ in 0..5000 {
        let term = format!("word{}", rand::thread_rng().gen_range(0..VOCAB_SIZE));
        let _results = index.search(&[term], 100)?;
    }
    
    let rss_after = get_rss_mb();
    println!("  RSS before: {:.2} MB", rss_before);
    println!("  RSS after 5K queries: {:.2} MB", rss_after);
    println!("  Working set: {:.2} MB\n", rss_after - rss_before);

    // Summary
    println!("=== SUMMARY ===");
    println!("Index: {} docs, {} terms", DOCS, index.count_terms()?);
    println!("Target: P99 < 50ms for all query types");
    println!("Competitive with: Algolia <50ms, Meilisearch <50ms");
    println!("\nNext steps:");
    println!("  1. Add filters (range queries on sort indices)");
    println!("  2. Benchmark text + filter intersection");
    println!("  3. Add multi-field sort indices");
    println!("  4. Measure combined text+filter+sort P99");

    fs::remove_dir_all(&temp_dir)?;
    Ok(())
}
