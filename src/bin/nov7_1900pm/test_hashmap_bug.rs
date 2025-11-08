use tantivy::collector::FacetCollector;
use tantivy::query::AllQuery;
use tantivy::schema::{Facet, FacetOptions, Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};
use std::collections::HashMap;

/// Simulates the BUGGY version of extract_facet_counts
fn extract_facet_counts_buggy(
    facet_counts: &tantivy::collector::FacetCounts,
    requests: &[(String, String)], // (field, path) tuples
) -> HashMap<String, Vec<(String, u64)>> {
    let mut result = HashMap::new();
    
    for (field, path) in requests {
        let counts: Vec<_> = facet_counts
            .get(path)
            .map(|(facet, count)| (facet.to_string(), count))
            .collect();
        
        // BUG: This overwrites previous entries for the same field
        result.insert(field.clone(), counts);
    }
    
    result
}

/// Simulates the FIXED version of extract_facet_counts
fn extract_facet_counts_fixed(
    facet_counts: &tantivy::collector::FacetCounts,
    requests: &[(String, String)], // (field, path) tuples
) -> HashMap<String, Vec<(String, u64)>> {
    let mut result = HashMap::new();
    
    for (field, path) in requests {
        let counts: Vec<_> = facet_counts
            .get(path)
            .map(|(facet, count)| (facet.to_string(), count))
            .collect();
        
        // FIX: Append to existing vec instead of overwriting
        result.entry(field.clone())
            .or_insert_with(Vec::new)
            .extend(counts);
    }
    
    result
}

fn main() -> tantivy::Result<()> {
    println!("=== Testing HashMap Overwrite Bug ===\n");

    // Setup
    let mut schema_builder = Schema::builder();
    let category_field = schema_builder.add_facet_field("category", FacetOptions::default());
    let title_field = schema_builder.add_text_field("title", TEXT | STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema);
    let mut index_writer: IndexWriter = index.writer(50_000_000)?;

    // Index documents
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

    // Prepare collector with multiple paths
    let mut collector = FacetCollector::for_field("category");
    collector.add_facet("/electronics");
    collector.add_facet("/books");
    
    let facet_counts = searcher.search(&AllQuery, &collector)?;

    // Simulate user requesting multiple paths for the same field
    let requests = vec![
        ("category".to_string(), "/electronics".to_string()),
        ("category".to_string(), "/books".to_string()),
    ];

    println!("Input requests:");
    for (field, path) in &requests {
        println!("  field: {}, path: {}", field, path);
    }
    println!();

    // Test buggy version
    println!("BUGGY version results:");
    let buggy_result = extract_facet_counts_buggy(&facet_counts, &requests);
    for (field, counts) in &buggy_result {
        println!("  field '{}' has {} facet(s):", field, counts.len());
        for (facet, count) in counts {
            println!("    {} = {}", facet, count);
        }
    }
    println!("  Total facets returned: {}", 
        buggy_result.values().map(|v| v.len()).sum::<usize>());
    println!();

    // Test fixed version
    println!("FIXED version results:");
    let fixed_result = extract_facet_counts_fixed(&facet_counts, &requests);
    for (field, counts) in &fixed_result {
        println!("  field '{}' has {} facet(s):", field, counts.len());
        for (facet, count) in counts {
            println!("    {} = {}", facet, count);
        }
    }
    println!("  Total facets returned: {}", 
        fixed_result.values().map(|v| v.len()).sum::<usize>());
    println!();

    // Verification
    let buggy_total: usize = buggy_result.values().map(|v| v.len()).sum();
    let fixed_total: usize = fixed_result.values().map(|v| v.len()).sum();

    println!("=== Analysis ===");
    println!("Expected: 4 facets total (2 electronics + 2 books)");
    println!("Buggy returned: {} facets (WRONG - overwrites /electronics with /books)", buggy_total);
    println!("Fixed returned: {} facets (CORRECT - merges both paths)", fixed_total);
    
    if buggy_total < fixed_total {
        println!("\n✓ Bug confirmed: HashMap overwrites earlier paths with later ones");
    }

    Ok(())
}