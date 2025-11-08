use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, RangeQuery};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use std::time::Instant;
use std::ops::Bound;

fn main() {
    println!("=== FILTER EXECUTION COST ISOLATION ===\n");
    
    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT);
    let body = schema_builder.add_text_field("body", TEXT);
    let price = schema_builder.add_u64_field("price", FAST | INDEXED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema.clone());
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    println!("Indexing 10,000 documents...");
    for i in 0..10_000 {
        let text = if i < 2000 {
            format!("Product {} laptop computer device", i)
        } else {
            format!("Product {} phone tablet gadget", i)
        };
        
        writer.add_document(doc!(
            title => text,
            body => format!("Description {}", i),
            price => (100 + (i % 900)) as u64,
        )).unwrap();
    }
    writer.commit().unwrap();
    
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();
    
    let query_parser = QueryParser::for_index(&index, vec![title, body]);
    let text_query = query_parser.parse_query("laptop").unwrap();

    // Measure filter-only cost at different cardinalities
    let test_cases = vec![
        (100, 150, 100),
        (100, 250, 500),
        (100, 420, 1200),
        (100, 600, 2000),
        (100, 800, 3000),
        (100, 999, 5000),
    ];

    println!("\n{:<12} {:<18} {:<18} {:<18}", "Filter Card", "Filter Time (ms)", "Text Time (ms)", "Score 20 (ms)");
    println!("{}", "-".repeat(70));

    for (min, max, expected_card) in test_cases {
        let filter_query = RangeQuery::new_u64_bounds(
            "price".to_string(),
            Bound::Included(min),
            Bound::Included(max),
        );

        // 1. Filter-only execution
        let mut filter_times = Vec::new();
        let mut actual_card = 0;
        for _ in 0..50 {
            let start = Instant::now();
            let results = searcher.search(&filter_query, &TopDocs::with_limit(10000)).unwrap();
            filter_times.push(start.elapsed().as_micros() as f64 / 1000.0);
            actual_card = results.len();
        }
        filter_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let filter_p99 = filter_times[49];

        // 2. Text-only execution
        let mut text_times = Vec::new();
        for _ in 0..50 {
            let start = Instant::now();
            let _ = searcher.search(&text_query, &TopDocs::with_limit(2000)).unwrap();
            text_times.push(start.elapsed().as_micros() as f64 / 1000.0);
        }
        text_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let text_p99 = text_times[49];

        // 3. Estimate scoring cost (score top 20 from filtered set)
        // This is fake - just fetch 20 docs to measure overhead
        let mut score_times = Vec::new();
        for _ in 0..50 {
            let start = Instant::now();
            let filter_results = searcher.search(&filter_query, &TopDocs::with_limit(actual_card)).unwrap();
            // Simulate scoring subset
            let _to_score = filter_results.iter().take(20).collect::<Vec<_>>();
            score_times.push(start.elapsed().as_micros() as f64 / 1000.0);
        }
        score_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let score_p99 = score_times[49];

        println!("{:<12} {:<18.3} {:<18.3} {:<18.3}", 
            format!("{}→{}", expected_card, actual_card), 
            filter_p99, 
            text_p99,
            score_p99
        );
    }

    println!("\n=== ANALYSIS ===");
    println!("Filter-first cost model: filter_time + (cardinality/2000) × text_time");
    println!("Text-first cost model: text_time + O(20) × filter_check");
    println!("\nThreshold = where models intersect");
    println!("If filter_time ~0.1ms and text_time ~0.3ms:");
    println!("  Filter-first: 0.1 + (N/2000)×0.3");
    println!("  Text-first: 0.3 + 0.01 = 0.31ms");
    println!("  Crossover: 0.1 + (N/2000)×0.3 = 0.31 → N ≈ 1400");
}