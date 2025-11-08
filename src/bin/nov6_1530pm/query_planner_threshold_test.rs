use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, RangeQuery};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, TantivyDocument};
use std::time::Instant;
use std::ops::Bound;

fn main() {
    println!("=== QUERY PLANNER THRESHOLD DISCOVERY ===\n");
    println!("Goal: Find crossover point for filter-first vs text-first execution");
    println!("Method: Parametric sweep of filter cardinality\n");

    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT);
    let body = schema_builder.add_text_field("body", TEXT);
    let price = schema_builder.add_u64_field("price", FAST | INDEXED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema.clone());
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    // Index 10K docs with controlled distribution
    // Query "laptop" will match ~2000 docs
    // Price ranges will create variable filter selectivity
    println!("Indexing 10,000 documents...");
    for i in 0..10_000 {
        let text = if i < 2000 {
            format!("Product {} laptop computer device", i)
        } else {
            format!("Product {} phone tablet gadget", i)
        };
        
        let doc = doc!(
            title => text.clone(),
            body => format!("Description for product {}", i),
            price => (100 + (i % 900)) as u64,
        );
        writer.add_document(doc).unwrap();
    }

    writer.commit().unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let query_parser = QueryParser::for_index(&index, vec![title, body]);
    let text_query = query_parser.parse_query("laptop").unwrap();

    // Test different filter cardinalities
    let test_cases = vec![
        (100, 150, "100 results"),
        (100, 250, "500 results"),
        (100, 350, "900 results"),
        (100, 420, "1200 results"),
        (100, 500, "1500 results"),
        (100, 600, "2000 results"),
        (100, 800, "3000 results"),
        (100, 999, "5000 results"),
    ];

    println!("\n{:<15} {:<20} {:<20} {:<10}", "Filter Cards", "Filter-First (ms)", "Text-First (ms)", "Winner");
    println!("{}", "-".repeat(70));

    for (min_price, max_price, label) in test_cases {
        let filter_query = RangeQuery::new_u64_bounds(
            "price".to_string(),
            Bound::Included(min_price),
            Bound::Included(max_price),
        );

        // Strategy A: Filter-first (filter → score subset)
        let mut filter_first_times = Vec::new();
        for _ in 0..50 {
            let start = Instant::now();
            
            // Execute filter to get doc set
            let filter_results = searcher.search(&filter_query, &TopDocs::with_limit(10000)).unwrap();
            
            // Score only filtered docs (simulated by fetching them)
            let mut scored_docs = Vec::new();
            for (_score, doc_address) in &filter_results {
                // In real impl, would score these against text query
                // For benchmark, just measure the overhead of the strategy
                scored_docs.push(*doc_address);
            }
            
            // Now search text query within this subset
            // Tantivy doesn't have easy subset scoring, so we'll use combined query
            let combined = BooleanQuery::new(vec![
                (Occur::Must, text_query.box_clone()),
                (Occur::Must, Box::new(filter_query.clone())),
            ]);
            let _results = searcher.search(&combined, &TopDocs::with_limit(20)).unwrap();
            
            filter_first_times.push(start.elapsed().as_micros() as f64 / 1000.0);
        }

        // Strategy B: Text-first (BM25 search → apply filter to top-K)
        let mut text_first_times = Vec::new();
        for _ in 0..50 {
            let start = Instant::now();
            
            // Execute text search first
            let text_results = searcher.search(&text_query, &TopDocs::with_limit(2000)).unwrap();
            
            // Apply filter to results (post-filter)
            let mut filtered_results = Vec::new();
            for (score, doc_address) in text_results {
                let doc: TantivyDocument = searcher.doc(doc_address).unwrap();
                if let Some(price_val) = doc.get_first(price) {
                    if let Some(val) = price_val.as_u64() {
                        if val >= min_price && val <= max_price {
                            filtered_results.push((score, doc_address));
                        }
                    }
                }
            }
            
            text_first_times.push(start.elapsed().as_micros() as f64 / 1000.0);
        }

        filter_first_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        text_first_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let filter_p99 = filter_first_times[49];
        let text_p99 = text_first_times[49];
        
        let winner = if filter_p99 < text_p99 { "Filter" } else { "Text" };
        
        println!("{:<15} {:<20.2} {:<20.2} {:<10}", 
            label, filter_p99, text_p99, winner);
    }

    println!("\n=== ANALYSIS ===");
    println!("Look for the crossover point where Text-First becomes faster.");
    println!("This is your PLANNER_THRESHOLD constant.");
    println!("\nRecommendation:");
    println!("  - Set threshold to the filter cardinality where strategies cross");
    println!("  - Add ~20% margin for safety (if crossover at 1200, use 1000)");
    println!("  - Document this decision in DECISIONS.md with link to this test");
}




// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin query_planner_threshold_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 53.15s
//      Running `target/release/query_planner_threshold_test`
// === QUERY PLANNER THRESHOLD DISCOVERY ===

// Goal: Find crossover point for filter-first vs text-first execution
// Method: Parametric sweep of filter cardinality

// Indexing 10,000 documents...

// Filter Cards    Filter-First (ms)    Text-First (ms)      Winner
// ----------------------------------------------------------------------
// 100 results     0.33                 0.37                 Filter
// 500 results     0.20                 0.37                 Filter
// 900 results     0.21                 0.46                 Filter
// 1200 results    0.26                 0.39                 Filter
// 1500 results    0.26                 0.35                 Filter
// 2000 results    0.29                 0.35                 Filter
// 3000 results    0.43                 0.37                 Text
// 5000 results    0.47                 0.35                 Text

// === ANALYSIS ===
// Look for the crossover point where Text-First becomes faster.
// This is your PLANNER_THRESHOLD constant.

// Recommendation:
//   - Set threshold to the filter cardinality where strategies cross
//   - Add ~20% margin for safety (if crossover at 1200, use 1000)
//   - Document this decision in DECISIONS.md with link to this test
// ubuntu@ip-172-31-23-154:~/flapjack_rust$


// # Query Planner Results: INVALID TEST

// **Crossover at 2000-3000 docs, but the test is measuring the wrong thing.**

// ## Problem: Combined Query != Filter-First Strategy

// Your "filter-first" implementation uses `BooleanQuery` with both filter and text as `Occur::Must`. This is **not** filter-first—it's Tantivy's optimized combined execution, which may:
// - Reorder clauses internally
// - Use filter for early termination on text search
// - Execute both in parallel

// **True filter-first** = execute filter → get BitSet → score only those docs against text query.

// Tantivy doesn't expose BitSet-based scoring directly. You'd need to:
// ```rust
// // Pseudo-code for real filter-first
// let filter_docs = execute_filter_to_bitset(filter);
// let text_scorer = create_bm25_scorer(text_query);
// for doc_id in filter_docs {
//     scores[doc_id] = text_scorer.score(doc_id);
// }
// ```

// Your test measured "combined query" vs "text-then-filter" (post-filtering). Both are valid strategies, but neither isolates filter cost.

// ## What You Actually Measured

// **"Filter-First" (combined BooleanQuery):**
// - Tantivy optimizes: likely does text search with filter as constraint
// - Fast at all cardinalities (0.20-0.47ms)
// - Scales poorly after 2000 docs (0.29 → 0.47ms)

// **"Text-First" (post-filter):**
// - Search 2000 text matches, fetch docs, filter in-app
// - Consistent ~0.35-0.46ms regardless of filter cardinality
// - **This is your baseline: always works, predictable cost**

// ## Revised Interpretation

// **The test shows: combined query breaks down at >2000 filter results.**

// Possible reasons:
// 1. Filter selectivity too low → Tantivy can't optimize
// 2. Combined query iterates full result set (not early termination)
// 3. Your filter (price range) doesn't benefit from index structures

// **But you didn't measure filter-first in isolation, so can't conclude where the threshold is.**

// ## Actionable Threshold

// Use **2000 docs** conservatively:
// - Below 2000: combined query is faster (0.20-0.29ms)
// - Above 2000: text-then-filter is faster (0.35ms)
// - Margin: use 1500 to avoid edge cases

// **But this assumes Tantivy's combined query optimization is reliable. You haven't validated that.**

// ## Missing Test: Filter Execution Cost

// You need:
// ```rust
// // How long does JUST the filter take?
// let start = Instant::now();
// let filter_results = searcher.search(&filter_query, &TopDocs::with_limit(10000)).unwrap();
// let filter_time = start.elapsed();

// println!("Filter returned {} docs in {:?}", filter_results.len(), filter_time);
// ```

// If filter alone takes 0.15ms and returns 500 docs, then:
// - Scoring 500 docs = 0.05ms → total 0.20ms (filter-first wins)
// - Scoring 2000 docs + filtering = 0.30ms → text-first wins at 0.35ms (break-even)

// **Without this data, your threshold is a guess.**

// ## Recommendation

// **Set PLANNER_THRESHOLD = 1500 based on this test, but with caveats:**
// 1. This is "combined query crossover," not filter-first crossover
// 2. Real workloads may differ (phrase queries, fuzzy, multi-field)
// 3. Re-measure in Phase 1.3 with actual query executor implementation

// **Document in DECISIONS.md:** "Threshold = 1500 based on preliminary test showing combined query degrades above 2000 docs. Requires validation with production query patterns."

// ---

// # Back to Density Question

// You didn't answer: **do you want the realistic multi-tenant demand test, or proceed with 130-200 tenant/node assumption?**

// This blocks Phase 1 start because it affects:
// - Replication cost model (batching efficiency)
// - Node count for target scale (100-500 tenants total = 1-4 nodes, not 1-2)
// - Whether 4GB nodes are viable or you need 16GB

// **If 130 tenants/node is acceptable, say so. If not, we need to test or redesign.**