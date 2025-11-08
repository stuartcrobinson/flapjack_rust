use tantivy::collector::FacetCollector;
use tantivy::query::{AllQuery, RangeQuery};
use std::ops::Bound;
use tantivy::schema::{Facet, FacetOptions, Schema, FAST, INDEXED, STORED};
use tantivy::{doc, Index, IndexWriter};
use std::collections::HashMap;

fn main() -> tantivy::Result<()> {
    println!("=== Comprehensive Faceting Test: Filters + Multi-Path ===\n");

    // Setup schema
    let mut schema_builder = Schema::builder();
    let category_field = schema_builder.add_facet_field("category", FacetOptions::default());
    let price_field = schema_builder.add_u64_field("price", INDEXED | FAST | STORED);
    let title_field = schema_builder.add_text_field("title", STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema);
    let mut index_writer: IndexWriter = index.writer(50_000_000)?;

    // Index electronics (prices 40-60)
    for i in 0..5 {
        index_writer.add_document(doc!(
            title_field => format!("Electronics {}", i),
            category_field => Facet::from("/electronics"),
            price_field => 40u64 + i * 5 // prices: 40, 45, 50, 55, 60
        ))?;
    }

    // Index books - CRITICAL: All prices BELOW 40 to test filter
    for i in 0..5 {
        index_writer.add_document(doc!(
            title_field => format!("Book {}", i),
            category_field => Facet::from("/books"),
            price_field => 10u64 + i * 5 // prices: 10, 15, 20, 25, 30
        ))?;
    }

    index_writer.commit()?;

    let reader = index.reader()?;
    let searcher = reader.searcher();

    println!("=== Bug 1: Do filters affect facet counts? ===\n");

    // Test without filter
    println!("Test 1a: No filter (AllQuery)");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        collector.add_facet("/books");
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        
        let electronics: Vec<_> = facet_counts.get("/electronics").collect();
        let books: Vec<_> = facet_counts.get("/books").collect();
        
        println!("  /electronics: {} documents", electronics.iter().map(|(_, c)| c).sum::<u64>());
        println!("  /books: {} documents", books.iter().map(|(_, c)| c).sum::<u64>());
    }

    // Test with price >= 40 filter
    println!("\nTest 1b: With filter (price >= 40)");
    {
        let price_query = RangeQuery::new_u64_bounds(
            "price".to_string(),
            Bound::Included(40u64),
            Bound::Unbounded,
        );
        
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        collector.add_facet("/books");
        
        let facet_counts = searcher.search(&price_query, &collector)?;
        
        let electronics: Vec<_> = facet_counts.get("/electronics").collect();
        let books: Vec<_> = facet_counts.get("/books").collect();
        
        println!("  /electronics: {} documents", electronics.iter().map(|(_, c)| c).sum::<u64>());
        println!("  /books: {} documents", books.iter().map(|(_, c)| c).sum::<u64>());
        
        let books_count: u64 = books.iter().map(|(_, c)| c).sum();
        
        if books_count == 0 {
            println!("  ✓ Filter works correctly: /books excluded (all have price < 40)");
        } else {
            println!("  ✗ UNEXPECTED: /books has {} docs despite filter", books_count);
            println!("    This suggests book prices might be >= 40");
        }
    }

    println!("\n=== Bug 2: HashMap overwrite with multi-path ===\n");

    // Simulate the buggy extraction
    println!("Simulating multi-path request for same field:");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        collector.add_facet("/books");
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        
        // Buggy version: overwrites
        let mut buggy_result: HashMap<String, Vec<(String, u64)>> = HashMap::new();
        
        println!("\nBUGGY extraction (overwrites):");
        
        // First request
        let electronics: Vec<_> = facet_counts
            .get("/electronics")
            .map(|(f, c)| (f.to_string(), c))
            .collect();
        buggy_result.insert("category".to_string(), electronics.clone());
        println!("  After processing /electronics: {} facets in map", 
            buggy_result.get("category").unwrap().len());
        
        // Second request - OVERWRITES
        let books: Vec<_> = facet_counts
            .get("/books")
            .map(|(f, c)| (f.to_string(), c))
            .collect();
        buggy_result.insert("category".to_string(), books.clone());
        println!("  After processing /books: {} facets in map", 
            buggy_result.get("category").unwrap().len());
        
        let buggy_total: usize = buggy_result.get("category").unwrap().len();
        println!("  Final count: {} (WRONG - lost /electronics data)", buggy_total);
        
        // Fixed version: appends
        let mut fixed_result: HashMap<String, Vec<(String, u64)>> = HashMap::new();
        
        println!("\nFIXED extraction (appends):");
        
        let electronics: Vec<_> = facet_counts
            .get("/electronics")
            .map(|(f, c)| (f.to_string(), c))
            .collect();
        fixed_result.entry("category".to_string())
            .or_insert_with(Vec::new)
            .extend(electronics);
        println!("  After processing /electronics: {} facets in map", 
            fixed_result.get("category").unwrap().len());
        
        let books: Vec<_> = facet_counts
            .get("/books")
            .map(|(f, c)| (f.to_string(), c))
            .collect();
        fixed_result.entry("category".to_string())
            .or_insert_with(Vec::new)
            .extend(books);
        println!("  After processing /books: {} facets in map", 
            fixed_result.get("category").unwrap().len());
        
        let fixed_total: usize = fixed_result.get("category").unwrap().len();
        println!("  Final count: {} (CORRECT - has both paths)", fixed_total);
        
        if fixed_total > buggy_total {
            println!("\n  ✓ Bug confirmed: Fixed version returns more facets");
        }
    }

    println!("\n=== Summary ===");
    println!("Bug 1: Facet collectors DO respect query filters");
    println!("       → Fix test data so books have price < 40");
    println!("Bug 2: HashMap.insert() overwrites previous values");
    println!("       → Use entry().or_insert_with().extend() to append");

    Ok(())
}