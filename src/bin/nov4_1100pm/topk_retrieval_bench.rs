// Top-K Retrieval Benchmark
// Tests: Early termination effectiveness for filtered search
//
// Critical questions:
// 1. Is exhaustive intersection needed or does early termination suffice?
// 2. When does filter-first beat text-first?
// 3. What's actual P99 latency for top-100 retrieval?

use anyhow::Result;
use rand::Rng;
use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct ScoredDoc {
    doc_id: u32,
    score: f32,
}

// Benchmark: Text-first with early termination
fn text_first_topk(
    text_results: &[ScoredDoc],  // Pre-sorted by BM25 score (descending)
    filter_set: &HashSet<u32>,    // Filter-matching doc IDs
    k: usize,
) -> (Vec<ScoredDoc>, usize) {
    let mut results = Vec::with_capacity(k);
    let mut checks = 0;

    for doc in text_results {
        checks += 1;
        if filter_set.contains(&doc.doc_id) {
            results.push(doc.clone());
            if results.len() >= k {
                break; // Early termination
            }
        }
    }

    (results, checks)
}

// Benchmark: Filter-first (score after filtering)
fn filter_first_topk(
    text_results: &[ScoredDoc],
    filter_set: &HashSet<u32>,
    k: usize,
) -> (Vec<ScoredDoc>, usize) {
    // Intersect first
    let mut filtered: Vec<ScoredDoc> = text_results
        .iter()
        .filter(|doc| filter_set.contains(&doc.doc_id))
        .cloned()
        .collect();

    // Sort by score
    filtered.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    
    // Take top-k
    filtered.truncate(k);
    
    let checks = text_results.len() + filtered.len(); // Intersection + sort cost proxy
    (filtered, checks)
}

// Benchmark: Exhaustive intersection (for comparison)
fn exhaustive_intersection(
    text_results: &[ScoredDoc],
    filter_set: &HashSet<u32>,
) -> Vec<u32> {
    text_results
        .iter()
        .filter(|doc| filter_set.contains(&doc.doc_id))
        .map(|doc| doc.doc_id)
        .collect()
}

fn generate_scored_results(n: usize, max_doc_id: u32) -> Vec<ScoredDoc> {
    let mut rng = rand::thread_rng();
    let mut results: Vec<ScoredDoc> = (0..n)
        .map(|_| ScoredDoc {
            doc_id: rng.gen_range(0..max_doc_id),
            score: rng.gen_range(1.0..10.0),
        })
        .collect();
    
    // Sort by score descending (BM25 pre-sorts)
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results
}

fn generate_filter_set(n: usize, max_doc_id: u32) -> HashSet<u32> {
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.gen_range(0..max_doc_id)).collect()
}

fn benchmark_scenario(
    name: &str,
    text_size: usize,
    filter_size: usize,
    overlap_ratio: f64,
    k: usize,
    iterations: usize,
) -> Result<()> {
    println!("\n=== {} ===", name);
    println!("Text results: {}, Filter results: {}, Overlap: {:.0}%, k={}",
             text_size, filter_size, overlap_ratio * 100.0, k);

    let max_doc_id = (text_size * 2) as u32;
    
    let mut text_first_times = Vec::new();
    let mut filter_first_times = Vec::new();
    let mut text_first_checks = Vec::new();
    let mut exhaustive_times = Vec::new();

    for _ in 0..iterations {
        // Generate data
        let text_results = generate_scored_results(text_size, max_doc_id);
        
        // Create filter set with controlled overlap
        let mut filter_set = HashSet::new();
        let target_overlap = (filter_size as f64 * overlap_ratio) as usize;
        
        // Add overlapping docs
        for doc in text_results.iter().take(target_overlap * 2) {
            filter_set.insert(doc.doc_id);
            if filter_set.len() >= target_overlap {
                break;
            }
        }
        
        // Fill rest with random docs
        while filter_set.len() < filter_size {
            filter_set.insert(rand::thread_rng().gen_range(0..max_doc_id));
        }

        // Benchmark text-first with early termination
        let start = Instant::now();
        let (_, checks) = text_first_topk(&text_results, &filter_set, k);
        text_first_times.push(start.elapsed());
        text_first_checks.push(checks);

        // Benchmark filter-first
        let start = Instant::now();
        let _ = filter_first_topk(&text_results, &filter_set, k);
        filter_first_times.push(start.elapsed());

        // Benchmark exhaustive intersection (for comparison)
        let start = Instant::now();
        let _ = exhaustive_intersection(&text_results, &filter_set);
        exhaustive_times.push(start.elapsed());
    }

    // Calculate statistics
    let calc_stats = |times: &mut Vec<Duration>| -> (Duration, Duration, Duration) {
        times.sort();
        let p50 = times[times.len() / 2];
        let p99 = times[(times.len() * 99) / 100];
        let avg = times.iter().sum::<Duration>() / times.len() as u32;
        (p50, p99, avg)
    };

    let (tf_p50, tf_p99, tf_avg) = calc_stats(&mut text_first_times);
    let (ff_p50, ff_p99, ff_avg) = calc_stats(&mut filter_first_times);
    let (ex_p50, ex_p99, ex_avg) = calc_stats(&mut exhaustive_times);

    let avg_checks = text_first_checks.iter().sum::<usize>() / text_first_checks.len();

    println!("\nText-first (early termination):");
    println!("  P50: {:.3}ms, P99: {:.3}ms, Avg: {:.3}ms",
             tf_p50.as_secs_f64() * 1000.0,
             tf_p99.as_secs_f64() * 1000.0,
             tf_avg.as_secs_f64() * 1000.0);
    println!("  Avg checks: {} (stopped early at {:.1}%)",
             avg_checks, (avg_checks as f64 / text_size as f64) * 100.0);

    println!("\nFilter-first (full intersection):");
    println!("  P50: {:.3}ms, P99: {:.3}ms, Avg: {:.3}ms",
             ff_p50.as_secs_f64() * 1000.0,
             ff_p99.as_secs_f64() * 1000.0,
             ff_avg.as_secs_f64() * 1000.0);

    println!("\nExhaustive intersection (no early stop):");
    println!("  P50: {:.3}ms, P99: {:.3}ms, Avg: {:.3}ms",
             ex_p50.as_secs_f64() * 1000.0,
             ex_p99.as_secs_f64() * 1000.0,
             ex_avg.as_secs_f64() * 1000.0);

    let speedup = ex_avg.as_secs_f64() / tf_avg.as_secs_f64();
    println!("\n✓ Early termination speedup: {:.1}x vs exhaustive", speedup);
    
    if tf_avg < ff_avg {
        println!("✓ Text-first wins by {:.1}x", ff_avg.as_secs_f64() / tf_avg.as_secs_f64());
    } else {
        println!("✓ Filter-first wins by {:.1}x", tf_avg.as_secs_f64() / ff_avg.as_secs_f64());
    }

    Ok(())
}

fn main() -> Result<()> {
    println!("Top-K Retrieval Benchmark");
    println!("=========================");
    println!("Goal: Validate <5ms P99 for top-100 retrieval\n");

    let iterations = 1000;
    let k = 100;

    // Scenario 1: High selectivity filter (90% overlap)
    benchmark_scenario(
        "Scenario 1: High Selectivity Filter",
        10_000,  // 10K text results
        9_000,   // 9K filter results
        0.90,    // 90% overlap
        k,
        iterations,
    )?;

    // Scenario 2: Medium selectivity (50% overlap)
    benchmark_scenario(
        "Scenario 2: Medium Selectivity Filter",
        10_000,
        5_000,
        0.50,
        k,
        iterations,
    )?;

    // Scenario 3: Low selectivity (10% overlap)
    benchmark_scenario(
        "Scenario 3: Low Selectivity Filter",
        10_000,
        1_000,
        0.10,
        k,
        iterations,
    )?;

    // Scenario 4: Ultra-selective filter (1% overlap)
    benchmark_scenario(
        "Scenario 4: Ultra-Selective Filter",
        10_000,
        100,
        0.01,
        k,
        iterations,
    )?;

    // Scenario 5: Boundary case - filter exactly equals k
    benchmark_scenario(
        "Scenario 5: Filter Size = k (Boundary)",
        10_000,
        100,
        0.90,
        k,
        iterations,
    )?;

    println!("\n=== Analysis ===");
    println!("1. If text-first P99 <5ms → Use text-first as default");
    println!("2. If filter-first wins when overlap <10% → Add query planner");
    println!("3. If early termination checks <1000 → No need for WAND/Roaring");
    println!("\nExpected Results:");
    println!("  - Text-first: <1ms P99 (early termination at ~200-500 checks)");
    println!("  - Filter-first: Wins only when filter is ultra-selective (<100 results)");
    println!("  - Exhaustive: 5-10x slower (no early stop)");

    Ok(())
}