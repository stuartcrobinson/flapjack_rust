use tantivy::collector::FacetCollector;
use tantivy::query::AllQuery;
use tantivy::schema::{Facet, FacetOptions, Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};

fn main() -> tantivy::Result<()> {
    println!("=== Testing Tantivy FacetCollector Multi-Path Behavior ===\n");

    // Setup schema
    let mut schema_builder = Schema::builder();
    let category_field = schema_builder.add_facet_field("category", FacetOptions::default());
    let title_field = schema_builder.add_text_field("title", TEXT | STORED);
    let schema = schema_builder.build();

    // Create in-memory index
    let index = Index::create_in_ram(schema);
    let mut index_writer: IndexWriter = index.writer(50_000_000)?;

    // Index documents with different category paths
    index_writer.add_document(doc!(
        title_field => "Laptop",
        category_field => Facet::from("/electronics/computers")
    ))?;
    
    index_writer.add_document(doc!(
        title_field => "Phone",
        category_field => Facet::from("/electronics/phones")
    ))?;
    
    index_writer.add_document(doc!(
        title_field => "Novel",
        category_field => Facet::from("/books/fiction")
    ))?;
    
    index_writer.add_document(doc!(
        title_field => "Textbook",
        category_field => Facet::from("/books/education")
    ))?;

    index_writer.commit()?;

    let reader = index.reader()?;
    let searcher = reader.searcher();

    // Test 1: Single add_facet call
    println!("Test 1: Single add_facet(\"/electronics\")");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        let electronics: Vec<_> = facet_counts.get("/electronics").collect();
        
        println!("  Results: {:?}", electronics);
        println!("  Count: {}\n", electronics.len());
    }

    // Test 2: Multiple add_facet calls - sibling paths
    println!("Test 2: Multiple add_facet calls - add_facet(\"/electronics\") + add_facet(\"/books\")");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        collector.add_facet("/books");
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        
        println!("  Getting /electronics:");
        let electronics: Vec<_> = facet_counts.get("/electronics").collect();
        println!("    {:?}", electronics);
        
        println!("  Getting /books:");
        let books: Vec<_> = facet_counts.get("/books").collect();
        println!("    {:?}", books);
        
        println!("  Total unique paths: {}\n", electronics.len() + books.len());
    }

    // Test 3: Root path
    println!("Test 3: add_facet(\"/\") - get all top-level categories");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/");
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        let root: Vec<_> = facet_counts.get("/").collect();
        
        println!("  Results: {:?}", root);
        println!("  Count: {}\n", root.len());
    }

    // Test 4: Prefix relationship (documented as forbidden)
    println!("Test 4: Prefix relationship - add_facet(\"/electronics\") + add_facet(\"/electronics/computers\")");
    println!("  (Documented as forbidden - testing behavior)");
    {
        let mut collector = FacetCollector::for_field("category");
        collector.add_facet("/electronics");
        collector.add_facet("/electronics/computers"); // This violates the documented constraint
        
        let facet_counts = searcher.search(&AllQuery, &collector)?;
        
        println!("  Getting /electronics:");
        let electronics: Vec<_> = facet_counts.get("/electronics").collect();
        println!("    {:?}", electronics);
        
        println!("  Getting /electronics/computers:");
        let computers: Vec<_> = facet_counts.get("/electronics/computers").collect();
        println!("    {:?}", computers);
    }

    println!("\n=== Key Findings ===");
    println!("1. Multiple add_facet() calls: Check if both paths return results");
    println!("2. Each get() call: Should return ONLY direct children of that path");
    println!("3. Prefix constraint: Observe actual behavior vs documentation");

    Ok(())
}