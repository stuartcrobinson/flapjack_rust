use std::path::PathBuf;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, TantivyDocument};
use std::time::Instant;
use std::collections::HashSet;

fn get_rss_mb() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let output = std::process::Command::new("ps")
            .args(&["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let kb: f64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        return Some(kb / 1024.0);
    }
    
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
                return Some(kb / 1024.0);
            }
        }
    }
    
    None
}

fn main() -> tantivy::Result<()> {
    println!("=== FST Overhead Test ===");
    println!("Measures memory increase from Tantivy's term dictionary\n");
    
    // Test 1: Minimal index (few unique terms)
    {
        println!("Test 1: Minimal vocabulary (100 unique terms)");
        let base_rss = get_rss_mb();
        
        let index_path = PathBuf::from("/tmp/tantivy_fst_minimal");
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000)?;
        
        let text_field = schema.get_field("text").unwrap();
        
        // 10K docs, only 100 unique terms (high term reuse)
        for i in 0..10_000 {
            let term = format!("term_{}", i % 100);
            writer.add_document(doc!(text_field => term))?;
        }
        
        writer.commit()?;
        
        // Force load by opening reader and searching
        let reader = index.reader()?;
        let searcher = reader.searcher();
        
        // Touch all terms
        for i in 0..100 {
            let query_parser = tantivy::query::QueryParser::for_index(&index, vec![text_field]);
            let query = query_parser.parse_query(&format!("term_{}", i))?;
            let _ = searcher.search(&query, &tantivy::collector::Count)?;
        }
        
        let after_rss = get_rss_mb();
        
        if let (Some(before), Some(after)) = (base_rss, after_rss) {
            println!("  RSS before: {:.1} MB", before);
            println!("  RSS after: {:.1} MB", after);
            println!("  Overhead: {:.1} MB", after - before);
            println!("  (100 terms in FST)\n");
        }
    }
    
    // Test 2: Large vocabulary (realistic)
    {
        println!("Test 2: Realistic vocabulary (~50K unique terms)");
        let base_rss = get_rss_mb();
        
        let index_path = PathBuf::from("/tmp/tantivy_fst_realistic");
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT);
        schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000)?;
        
        let title_field = schema.get_field("title").unwrap();
        let body_field = schema.get_field("body").unwrap();
        
        // Track actual unique terms
        let mut unique_terms = HashSet::new();
        
        // 10K docs with varied vocabulary
        for i in 0..10_000 {
            let title = format!("Product {} {} {}", 
                ["Laptop", "Phone", "Tablet", "Monitor", "Keyboard"][i % 5],
                ["Gaming", "Professional", "Budget", "Premium", "Wireless"][i % 5],
                i
            );
            
            let body = format!(
                "{} {} {} {} {} {}",
                ["high", "low", "medium"][i % 3],
                ["performance", "quality", "price"][i % 3],
                ["excellent", "good", "acceptable"][i % 3],
                ["features", "design", "build"][i % 3],
                ["advanced", "basic", "standard"][i % 3],
                i
            );
            
            for word in title.split_whitespace().chain(body.split_whitespace()) {
                unique_terms.insert(word.to_lowercase());
            }
            
            writer.add_document(doc!(
                title_field => title,
                body_field => body
            ))?;
        }
        
        writer.commit()?;
        
        println!("  Unique terms: {}", unique_terms.len());
        
        // Force load
        let reader = index.reader()?;
        let searcher = reader.searcher();
        
        // Sample queries
        for term in ["laptop", "gaming", "high", "performance", "excellent"].iter() {
            let query_parser = tantivy::query::QueryParser::for_index(&index, vec![title_field, body_field]);
            let query = query_parser.parse_query(term)?;
            let _ = searcher.search(&query, &tantivy::collector::Count)?;
        }
        
        let after_rss = get_rss_mb();
        
        if let (Some(before), Some(after)) = (base_rss, after_rss) {
            println!("  RSS before: {:.1} MB", before);
            println!("  RSS after: {:.1} MB", after);
            println!("  Overhead: {:.1} MB", after - before);
            println!("  Bytes per term: {:.1}", (after - before) * 1024.0 * 1024.0 / unique_terms.len() as f64);
            println!();
        }
    }
    
    // Test 3: Pathological (massive vocabulary)
    {
        println!("Test 3: Large vocabulary (200K unique terms)");
        let base_rss = get_rss_mb();
        
        let index_path = PathBuf::from("/tmp/tantivy_fst_large");
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000)?;
        
        let text_field = schema.get_field("text").unwrap();
        
        // 10K docs, each with 20 unique terms
        for i in 0..10_000 {
            let mut terms = Vec::new();
            for j in 0..20 {
                terms.push(format!("term_{}_{}", i, j));
            }
            writer.add_document(doc!(text_field => terms.join(" ")))?;
        }
        
        writer.commit()?;
        
        let reader = index.reader()?;
        let searcher = reader.searcher();
        
        // Sample query
        let query_parser = tantivy::query::QueryParser::for_index(&index, vec![text_field]);
        let query = query_parser.parse_query("term_5000_10")?;
        let _ = searcher.search(&query, &tantivy::collector::Count)?;
        
        let after_rss = get_rss_mb();
        
        if let (Some(before), Some(after)) = (base_rss, after_rss) {
            println!("  RSS before: {:.1} MB", before);
            println!("  RSS after: {:.1} MB", after);
            println!("  Overhead: {:.1} MB", after - before);
            println!("  Bytes per term: {:.1}", (after - before) * 1024.0 * 1024.0 / 200_000.0);
        }
    }
    
    println!("\n=== Conclusion ===");
    println!("FST overhead scales with unique term count, not document count.");
    println!("If Test 2 shows >5MB for 50K terms, FST is significant overhead.");
    println!("If <2MB, then FST is negligible vs LMDB's B-tree metadata.");
    
    Ok(())
}