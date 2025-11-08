// /tmp/faceting_viability_test.rs

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, TantivyDocument};
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    println!("=== FACETING VIABILITY TEST ===\n");
    println!("Goal: Validate query-time aggregation on 10K docs with 1000 results");
    println!("Pass condition: P99 <50ms for 5 facet fields\n");

    let mut schema_builder = Schema::builder();
    let title = schema_builder.add_text_field("title", TEXT | STORED);
    let body = schema_builder.add_text_field("body", TEXT);
    let price = schema_builder.add_u64_field("price", FAST | STORED);
    let category = schema_builder.add_text_field("category", STRING | STORED);
    let brand = schema_builder.add_text_field("brand", STRING | STORED);
    let rating = schema_builder.add_u64_field("rating", FAST | STORED);
    let availability = schema_builder.add_text_field("availability", STRING | STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema.clone());
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    // Index 10K docs
    // Categories: 100 unique values
    // Brands: 50 unique values
    // Ratings: 1-5
    // Availability: in_stock, out_of_stock, preorder
    println!("Indexing 10,000 documents...");
    let categories: Vec<String> = (0..100).map(|i| format!("category_{}", i)).collect();
    let brands: Vec<String> = (0..50).map(|i| format!("brand_{}", i)).collect();
    let availability_opts = vec!["in_stock", "out_of_stock", "preorder"];

    for i in 0..10_000 {
        let doc = doc!(
            title => format!("Product {} laptop computer device", i),
            body => "This is a great electronic device for computing and productivity work",
            price => (100 + (i % 900)) as u64,
            category => categories[i % 100].clone(),
            brand => brands[i % 50].clone(),
            rating => (1 + (i % 5)) as u64,
            availability => availability_opts[i % 3].to_string(),
        );
        writer.add_document(doc).unwrap();
    }

    println!("Committing...");
    writer.commit().unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    // Query: "laptop" should match ~10K docs (all of them)
    // Then we'll test aggregation on subsets
    let query_parser = QueryParser::for_index(&index, vec![title, body]);
    let query = query_parser.parse_query("laptop").unwrap();

    // Test 1: Aggregate 1000 results
    println!("\n--- Test 1: Aggregating 1000 results ---");
    let mut latencies_1000 = Vec::new();

    for _ in 0..100 {
        let start = Instant::now();
        
        let top_docs = searcher.search(&query, &TopDocs::with_limit(1000)).unwrap();
        
        // Aggregate 5 facet fields
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        let mut brand_counts: HashMap<String, usize> = HashMap::new();
        let mut rating_counts: HashMap<u64, usize> = HashMap::new();
        let mut availability_counts: HashMap<String, usize> = HashMap::new();
        let mut price_buckets: HashMap<String, usize> = HashMap::new();

        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).unwrap();
            
            if let Some(cat) = doc.get_first(category) {
                if let Some(text) = cat.as_str() {
                    *category_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(br) = doc.get_first(brand) {
                if let Some(text) = br.as_str() {
                    *brand_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(rat) = doc.get_first(rating) {
                if let Some(val) = rat.as_u64() {
                    *rating_counts.entry(val).or_insert(0) += 1;
                }
            }
            
            if let Some(avail) = doc.get_first(availability) {
                if let Some(text) = avail.as_str() {
                    *availability_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(pr) = doc.get_first(price) {
                if let Some(val) = pr.as_u64() {
                    let bucket = match val {
                        0..=200 => "0-200",
                        201..=400 => "201-400",
                        401..=600 => "401-600",
                        601..=800 => "601-800",
                        _ => "801+",
                    };
                    *price_buckets.entry(bucket.to_string()).or_insert(0) += 1;
                }
            }
        }
        
        let elapsed = start.elapsed();
        latencies_1000.push(elapsed.as_micros() as f64 / 1000.0);
    }

    latencies_1000.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50_1000 = latencies_1000[49];
    let p99_1000 = latencies_1000[98];
    
    println!("Results (1000 docs):");
    println!("  P50: {:.2}ms", p50_1000);
    println!("  P99: {:.2}ms", p99_1000);

    // Test 2: Aggregate 10K results (worst case)
    println!("\n--- Test 2: Aggregating 10K results ---");
    let mut latencies_10k = Vec::new();

    for _ in 0..100 {
        let start = Instant::now();
        
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10000)).unwrap();
        
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        let mut brand_counts: HashMap<String, usize> = HashMap::new();
        let mut rating_counts: HashMap<u64, usize> = HashMap::new();
        let mut availability_counts: HashMap<String, usize> = HashMap::new();
        let mut price_buckets: HashMap<String, usize> = HashMap::new();

        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).unwrap();
            
            if let Some(cat) = doc.get_first(category) {
                if let Some(text) = cat.as_str() {
                    *category_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(br) = doc.get_first(brand) {
                if let Some(text) = br.as_str() {
                    *brand_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(rat) = doc.get_first(rating) {
                if let Some(val) = rat.as_u64() {
                    *rating_counts.entry(val).or_insert(0) += 1;
                }
            }
            
            if let Some(avail) = doc.get_first(availability) {
                if let Some(text) = avail.as_str() {
                    *availability_counts.entry(text.to_string()).or_insert(0) += 1;
                }
            }
            
            if let Some(pr) = doc.get_first(price) {
                if let Some(val) = pr.as_u64() {
                    let bucket = match val {
                        0..=200 => "0-200",
                        201..=400 => "201-400",
                        401..=600 => "401-600",
                        601..=800 => "601-800",
                        _ => "801+",
                    };
                    *price_buckets.entry(bucket.to_string()).or_insert(0) += 1;
                }
            }
        }
        
        let elapsed = start.elapsed();
        latencies_10k.push(elapsed.as_micros() as f64 / 1000.0);
    }

    latencies_10k.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50_10k = latencies_10k[49];
    let p99_10k = latencies_10k[98];
    
    println!("Results (10K docs):");
    println!("  P50: {:.2}ms", p50_10k);
    println!("  P99: {:.2}ms", p99_10k);

    // Verdict
    println!("\n=== VERDICT ===");
    if p99_1000 < 50.0 {
        println!("✓ PASS: Query-time aggregation viable for typical queries (1000 results)");
        println!("  Recommendation: Use query-time aggregation in Phase 1");
    } else {
        println!("✗ FAIL: Query-time aggregation too slow");
        println!("  Recommendation: Need pre-built facet indices (changes Phase 1.1 schema)");
    }

    if p99_10k < 100.0 {
        println!("✓ PASS: Even worst-case (10K results) is acceptable");
    } else {
        println!("⚠ WARNING: Large result sets will need optimization");
        println!("  Consider: Result limit enforcement, pre-built indices, or pagination");
    }
}
