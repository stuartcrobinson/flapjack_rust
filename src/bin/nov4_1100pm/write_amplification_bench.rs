// Write Amplification Benchmark
// Tests: Document write latency with multiple B-tree indices (BM25 + filters)
//
// Critical questions:
// 1. Does 5 filter indices blow the <15ms write target?
// 2. What's the batch size sweet spot for LMDB commits?
// 3. Memory spike during B-tree rebalancing?

use anyhow::Result;
use heed::types::*;
use heed::{Database, EnvOpenOptions};
use rand::Rng;
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

// Simulated document with filter fields
#[derive(Debug, Clone)]
struct Document {
    id: u32,
    text: String,
    price: u32,       // Filter index 1
    date: u64,        // Filter index 2 (timestamp)
    category_id: u16, // Filter index 3
    rating: u8,       // Filter index 4 (0-100)
    stock: u16,       // Filter index 5
}

struct MultiIndexStore {
    env: heed::Env,
    // BM25 posting lists (simplified - just doc_id lists per term)
    bm25_postings: Database<Str, SerdeBincode<Vec<u32>>>,
    // Filter indices (big-endian for range queries)
    price_idx: Database<U32<byteorder::BigEndian>, U32<byteorder::NativeEndian>>,
    date_idx: Database<U64<byteorder::BigEndian>, U32<byteorder::NativeEndian>>,
    category_idx: Database<U16<byteorder::BigEndian>, U32<byteorder::NativeEndian>>,
    rating_idx: Database<U8, U32<byteorder::NativeEndian>>,
    stock_idx: Database<U16<byteorder::BigEndian>, U32<byteorder::NativeEndian>>,
}

impl MultiIndexStore {
    fn create(path: &str) -> Result<Self> {
        fs::create_dir_all(path)?;
        
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(1024 * 1024 * 1024) // 1GB
                .max_dbs(10)
                .open(path)?
        };

        let mut wtxn = env.write_txn()?;
        let bm25_postings = env.create_database(&mut wtxn, Some("bm25_postings"))?;
        let price_idx = env.create_database(&mut wtxn, Some("price_idx"))?;
        let date_idx = env.create_database(&mut wtxn, Some("date_idx"))?;
        let category_idx = env.create_database(&mut wtxn, Some("category_idx"))?;
        let rating_idx = env.create_database(&mut wtxn, Some("rating_idx"))?;
        let stock_idx = env.create_database(&mut wtxn, Some("stock_idx"))?;
        wtxn.commit()?;

        Ok(Self {
            env,
            bm25_postings,
            price_idx,
            date_idx,
            category_idx,
            rating_idx,
            stock_idx,
        })
    }

    fn batch_index_documents(&self, docs: &[Document]) -> Result<Duration> {
        let start = Instant::now();
        let mut wtxn = self.env.write_txn()?;
        
        // Build term → doc_id map across all docs in batch
        let mut term_postings: HashMap<String, Vec<u32>> = HashMap::new();
        
        for doc in docs {
            // Extract terms from text
            let terms: Vec<String> = doc.text
                .split_whitespace()
                .map(|s| s.to_lowercase())
                .collect();

            for term in terms {
                term_postings.entry(term)
                    .or_default()
                    .push(doc.id);
            }

            // Update filter indices
            self.price_idx.put(&mut wtxn, &doc.price, &doc.id)?;
            self.date_idx.put(&mut wtxn, &doc.date, &doc.id)?;
            self.category_idx.put(&mut wtxn, &doc.category_id, &doc.id)?;
            self.rating_idx.put(&mut wtxn, &doc.rating, &doc.id)?;
            self.stock_idx.put(&mut wtxn, &doc.stock, &doc.id)?;
        }

        // Merge with existing posting lists and write once per term
        for (term, new_doc_ids) in term_postings {
            let mut postings = self.bm25_postings
                .get(&wtxn, &term)?
                .unwrap_or_default();
            
            postings.extend(new_doc_ids);
            postings.sort_unstable();
            postings.dedup();
            
            self.bm25_postings.put(&mut wtxn, &term, &postings)?;
        }

        wtxn.commit()?;
        Ok(start.elapsed())
    }
}

fn generate_document(id: u32, vocab: &[String]) -> Document {
    let mut rng = rand::thread_rng();
    
    // Generate text with 50-150 tokens
    let text_len = rng.gen_range(50..150);
    let text = (0..text_len)
        .map(|_| vocab[rng.gen_range(0..vocab.len())].clone())
        .collect::<Vec<_>>()
        .join(" ");

    Document {
        id,
        text,
        price: rng.gen_range(100..10000),
        date: 1700000000 + rng.gen_range(0..31536000),
        category_id: rng.gen_range(0..100),
        rating: rng.gen_range(0..101),
        stock: rng.gen_range(0..1000),
    }
}

fn benchmark_batch_writes(n_docs: usize, batch_sizes: &[usize]) -> Result<()> {
    println!("\n=== Batch Write Benchmark ===");
    
    let vocab: Vec<String> = (0..5000)
        .map(|i| format!("word{}", i))
        .collect();

    for &batch_size in batch_sizes {
        let path = format!("/tmp/flapjack_write_bench_batch_{}", batch_size);
        let _ = fs::remove_dir_all(&path);
        let store = MultiIndexStore::create(&path)?;

        println!("\nBatch size: {}", batch_size);
        
        let mut batches = Vec::new();
        let mut current_batch = Vec::new();
        
        for i in 0..n_docs {
            current_batch.push(generate_document(i as u32, &vocab));
            
            if current_batch.len() == batch_size {
                batches.push(current_batch.clone());
                current_batch.clear();
            }
        }
        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        let mut latencies = Vec::new();
        let mut total_docs = 0;
        
        for (i, batch) in batches.iter().enumerate() {
            let duration = store.batch_index_documents(batch)?;
            latencies.push(duration);
            total_docs += batch.len();
            
            if (i + 1) % 10 == 0 {
                print!("\rIndexed {} docs in {} batches", total_docs, i + 1);
            }
        }
        println!();

        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p99 = latencies[(latencies.len() * 99) / 100];
        let total: Duration = latencies.iter().sum();
        let avg_per_doc = total.as_secs_f64() / n_docs as f64;

        println!("  Batch commit P50: {:.2}ms", p50.as_secs_f64() * 1000.0);
        println!("  Batch commit P99: {:.2}ms", p99.as_secs_f64() * 1000.0);
        println!("  Avg per doc: {:.3}ms", avg_per_doc * 1000.0);
        println!("  Total throughput: {:.0} docs/sec", n_docs as f64 / total.as_secs_f64());
        
        // Pass/fail criteria
        if batch_size == 1 {
            if p99.as_secs_f64() * 1000.0 > 15.0 {
                println!("  ❌ FAIL: Single-doc commit P99 >{:.0}ms (target <15ms)", p99.as_secs_f64() * 1000.0);
            } else {
                println!("  ✅ PASS: Single-doc commit under budget");
            }
        } else if batch_size >= 50 {
            if avg_per_doc * 1000.0 > 2.0 {
                println!("  ❌ FAIL: Batch avg >{:.2}ms/doc (target <2ms)", avg_per_doc * 1000.0);
            } else {
                println!("  ✅ PASS: Batch throughput acceptable");
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    println!("Write Amplification Benchmark");
    println!("==============================");
    println!("Testing: BM25 + 5 filter indices");
    println!("Goal: Validate <15ms P99 write latency\n");

    let batch_sizes = vec![1, 10, 50, 100, 500];
    benchmark_batch_writes(5000, &batch_sizes)?;

    println!("\n=== Analysis ===");
    println!("1. If single write P99 >15ms → FAIL (need NOSYNC mode or fewer indices)");
    println!("2. If batch(50) avg >2ms/doc → FAIL (too slow for real-time indexing)");
    println!("3. Optimal batch size = highest throughput with <20ms P99 commit");
    println!("\nExpectation:");
    println!("  Single: ~5-10ms P99 (6 B-tree updates + 2 fsyncs)");
    println!("  Batch(50): ~0.5-1ms per doc (amortized fsync cost)");
    println!("  Batch(100+): Diminishing returns, higher latency variance");

    Ok(())
}