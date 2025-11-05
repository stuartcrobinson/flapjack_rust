// Filter Selectivity Test
// Tests: When does filter-first beat text-first?
//
// Critical questions:
// 1. At what filter cardinality should we switch strategies?
// 2. Can we estimate selectivity from B-tree bounds check?
// 3. Is a simple heuristic sufficient or need cost model?

use anyhow::Result;
use heed::types::*;
use heed::{Database, EnvOpenOptions};
use rand::Rng;
use std::collections::HashSet;
use std::fs;
use std::time::{Duration, Instant};

struct TestIndex {
    env: heed::Env,
    // Text search results (pre-scored)
    text_scores: Database<U32<byteorder::NativeEndian>, SerdeBincode<f32>>,
    // Filter index
    price_idx: Database<U32<byteorder::BigEndian>, U32<byteorder::NativeEndian>>,
}

impl TestIndex {
    fn create(path: &str, n_docs: u32) -> Result<Self> {
        fs::create_dir_all(path)?;
        
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(512 * 1024 * 1024) // 512MB
                .max_dbs(5)
                .open(path)?
        };

        let mut wtxn = env.write_txn()?;
        let text_scores = env.create_database(&mut wtxn, Some("text_scores"))?;
        let price_idx = env.create_database(&mut wtxn, Some("price_idx"))?;

        // Populate with test data
        let mut rng = rand::thread_rng();
        
        for doc_id in 0..n_docs {
            // Text score (BM25 simulation)
            let score = rng.gen_range(1.0..10.0_f32);
            text_scores.put(&mut wtxn, &doc_id, &score)?;
            
            // Price (filter index)
            let price = rng.gen_range(100..10000_u32);
            price_idx.put(&mut wtxn, &price, &doc_id)?;
        }

        wtxn.commit()?;
        Ok(Self { env, text_scores, price_idx })
    }

    // Text-first: BM25 → filter → top-k
    fn query_text_first(&self, price_min: u32, price_max: u32, k: usize) -> Result<(Vec<u32>, Duration)> {
        let rtxn = self.env.read_txn()?;
        
        // Get filter set
        let filter_start = Instant::now();
        let mut filter_set = HashSet::new();
        for result in self.price_idx.range(&rtxn, &(price_min..=price_max))? {
            let (_, doc_id) = result?;
            filter_set.insert(doc_id);
        }
        let filter_time = filter_start.elapsed();

        // Get all text results sorted by score
        let mut text_results: Vec<(u32, f32)> = Vec::new();
        for result in self.text_scores.iter(&rtxn)? {
            let (doc_id, score) = result?;
            text_results.push((doc_id, score));
        }
        text_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Early termination: iterate until k matches
        let mut results = Vec::new();
        for (doc_id, _score) in text_results {
            if filter_set.contains(&doc_id) {
                results.push(doc_id);
                if results.len() >= k {
                    break;
                }
            }
        }

        Ok((results, filter_time))
    }

    // Filter-first: filter → score → sort → top-k
    fn query_filter_first(&self, price_min: u32, price_max: u32, k: usize) -> Result<(Vec<u32>, Duration)> {
        let start = Instant::now();
        let rtxn = self.env.read_txn()?;
        
        // Get filtered docs
        let mut filtered_docs = Vec::new();
        for result in self.price_idx.range(&rtxn, &(price_min..=price_max))? {
            let (_, doc_id) = result?;
            filtered_docs.push(doc_id);
        }
        let filter_time = start.elapsed();

        // Score filtered docs
        let mut scored: Vec<(u32, f32)> = Vec::new();
        for doc_id in filtered_docs {
            if let Some(score) = self.text_scores.get(&rtxn, &doc_id)? {
                scored.push((doc_id, score));
            }
        }

        // Sort and take top-k
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        scored.truncate(k);

        let results: Vec<u32> = scored.into_iter().map(|(id, _)| id).collect();
        Ok((results, filter_time))
    }

    // Estimate filter selectivity from B-tree (without full scan)
    fn estimate_filter_cardinality(&self, price_min: u32, price_max: u32) -> Result<usize> {
        let rtxn = self.env.read_txn()?;
        
        // Count entries in range (actual cardinality)
        let count = self.price_idx
            .range(&rtxn, &(price_min..=price_max))?
            .count();
        
        Ok(count)
    }

    fn total_docs(&self) -> Result<usize> {
        let rtxn = self.env.read_txn()?;
        Ok(self.text_scores.len(&rtxn)? as usize)
    }
}

fn test_crossover_point(n_docs: u32) -> Result<()> {
    println!("\n=== Crossover Point Test ===");
    println!("Total docs: {}", n_docs);
    println!("Finding filter selectivity where filter-first wins\n");

    let path = "/tmp/flapjack_selectivity_test";
    let _ = fs::remove_dir_all(path);
    let index = TestIndex::create(path, n_docs)?;

    let k = 100;
    let trials = 50;

    // Test different filter selectivities
    let selectivities = vec![
        (100, 1000),    // Ultra-selective: ~900 results (9%)
        (100, 2000),    // High: ~1900 (19%)
        (100, 5000),    // Medium: ~4900 (49%)
        (100, 8000),    // Low: ~7900 (79%)
        (100, 9900),    // Barely selective: ~9800 (98%)
    ];

    for (price_min, price_max) in selectivities {
        let cardinality = index.estimate_filter_cardinality(price_min, price_max)?;
        let selectivity = (cardinality as f64 / n_docs as f64) * 100.0;
        
        println!("Filter range [{}, {}]:", price_min, price_max);
        println!("  Cardinality: {} ({:.1}% selective)", cardinality, selectivity);

        let mut text_first_times = Vec::new();
        let mut filter_first_times = Vec::new();

        for _ in 0..trials {
            // Text-first
            let start = Instant::now();
            let _ = index.query_text_first(price_min, price_max, k)?;
            text_first_times.push(start.elapsed());

            // Filter-first
            let start = Instant::now();
            let _ = index.query_filter_first(price_min, price_max, k)?;
            filter_first_times.push(start.elapsed());
        }

        let tf_avg: Duration = text_first_times.iter().sum::<Duration>() / trials as u32;
        let ff_avg: Duration = filter_first_times.iter().sum::<Duration>() / trials as u32;

        println!("  Text-first:   {:.3}ms", tf_avg.as_secs_f64() * 1000.0);
        println!("  Filter-first: {:.3}ms", ff_avg.as_secs_f64() * 1000.0);

        if ff_avg < tf_avg {
            let speedup = tf_avg.as_secs_f64() / ff_avg.as_secs_f64();
            println!("  ✓ Filter-first WINS by {:.2}x", speedup);
        } else {
            let speedup = ff_avg.as_secs_f64() / tf_avg.as_secs_f64();
            println!("  ✓ Text-first wins by {:.2}x", speedup);
        }
    }

    Ok(())
}

fn test_query_planner_heuristic(n_docs: u32) -> Result<()> {
    println!("\n=== Query Planner Heuristic Test ===");
    println!("Determining optimal decision threshold\n");

    let path = "/tmp/flapjack_planner_test";
    let _ = fs::remove_dir_all(path);
    let index = TestIndex::create(path, n_docs)?;

    let k = 100;
    let trials = 100;

    // Sweep across selectivity spectrum
    let mut crossover_selectivity = None;
    
    for selectivity_pct in (1..=99).step_by(5) {
        let target_cardinality = (n_docs as f64 * (selectivity_pct as f64 / 100.0)) as u32;
        
        // Find price range that gives ~target_cardinality
        let price_min = 100;
        let price_max = price_min + (target_cardinality * 10); // Approximation
        
        let actual_cardinality = index.estimate_filter_cardinality(price_min, price_max)?;
        
        let mut tf_times = Vec::new();
        let mut ff_times = Vec::new();

        for _ in 0..trials {
            let start = Instant::now();
            let _ = index.query_text_first(price_min, price_max, k)?;
            tf_times.push(start.elapsed());

            let start = Instant::now();
            let _ = index.query_filter_first(price_min, price_max, k)?;
            ff_times.push(start.elapsed());
        }

        let tf_avg: Duration = tf_times.iter().sum::<Duration>() / trials as u32;
        let ff_avg: Duration = ff_times.iter().sum::<Duration>() / trials as u32;

        // Detect crossover
        if ff_avg < tf_avg && crossover_selectivity.is_none() {
            crossover_selectivity = Some((selectivity_pct, actual_cardinality));
            println!("✓ CROSSOVER DETECTED at {}% selectivity ({} docs)", 
                     selectivity_pct, actual_cardinality);
            println!("  Text-first:   {:.3}ms", tf_avg.as_secs_f64() * 1000.0);
            println!("  Filter-first: {:.3}ms", ff_avg.as_secs_f64() * 1000.0);
        }
    }

    if let Some((_selectivity, cardinality)) = crossover_selectivity {
        println!("\n=== Recommended Heuristic ===");
        println!("if filter_cardinality < {} {{", cardinality);
        println!("    execute_filter_first()");
        println!("}} else {{");
        println!("    execute_text_first()  // with early termination");
        println!("}}");
    } else {
        println!("\n✓ Text-first always wins - no query planner needed!");
    }

    Ok(())
}

fn main() -> Result<()> {
    println!("Filter Selectivity & Query Planning Test");
    println!("=========================================");
    println!("Goal: Determine when to use filter-first vs text-first\n");

    let n_docs = 10_000;

    // Test 1: Find crossover point
    test_crossover_point(n_docs)?;

    // Test 2: Derive query planner heuristic
    test_query_planner_heuristic(n_docs)?;

    println!("\n=== Analysis ===");
    println!("1. If text-first always wins → Simple: always use text-first");
    println!("2. If filter-first wins at <5% selectivity → Add threshold check");
    println!("3. If crossover >20% selectivity → Query planner not worth complexity");
    println!("\nExpected Result:");
    println!("  Crossover at ~5-10% selectivity (500-1000 filter results)");
    println!("  Simple heuristic: filter_cardinality < 1000");

    Ok(())
}