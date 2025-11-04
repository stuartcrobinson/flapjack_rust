use std::path::PathBuf;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexReader, IndexWriter, TantivyDocument};
use std::time::Instant;

fn get_rss_mb() -> Option<f64> {
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
    
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    None
}

fn create_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    schema_builder.add_i64_field("price", INDEXED | STORED | FAST);
    schema_builder.add_text_field("tenant_id", STRING | STORED);
    schema_builder.build()
}

fn test_1_empty_index_overhead() -> tantivy::Result<()> {
    println!("\n=== Test 1: Empty Index Overhead ===");
    let base_rss = get_rss_mb();
    
    let mut indices = Vec::new();
    let num_indices = 100;
    
    for i in 0..num_indices {
        let index_path = PathBuf::from(format!("/tmp/tantivy_test_empty_{}", i));
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let schema = create_schema();
        let index = Index::create_in_dir(&index_path, schema)?;
        indices.push(index);
    }
    
    let after_rss = get_rss_mb();
    
    println!("Indices created: {}", num_indices);
    if let (Some(before), Some(after)) = (base_rss, after_rss) {
        let overhead = (after - before) / num_indices as f64;
        println!("RSS before: {:.1} MB", before);
        println!("RSS after: {:.1} MB", after);
        println!("Per-index overhead: {:.1} MB", overhead);
    } else {
        println!("⚠️  RSS measurement failed");
    }
    
    Ok(())
}

fn test_2_realistic_overhead() -> tantivy::Result<()> {
    println!("\n=== Test 2: Realistic Overhead (20 indices × 10K docs) ===");
    let base_rss = get_rss_mb();
    
    let num_indices = 20;
    let docs_per_index = 10_000;
    let start = Instant::now();
    
    for idx in 0..num_indices {
        let index_path = PathBuf::from(format!("/tmp/tantivy_test_realistic_{}", idx));
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let schema = create_schema();
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000)?;
        
        let title_field = schema.get_field("title").unwrap();
        let body_field = schema.get_field("body").unwrap();
        let price_field = schema.get_field("price").unwrap();
        let tenant_field = schema.get_field("tenant_id").unwrap();
        
        for doc_id in 0..docs_per_index {
            writer.add_document(doc!(
                title_field => format!("Product {}", doc_id),
                body_field => "High quality product with excellent features and benefits",
                price_field => (1000 + (doc_id % 5000)) as i64,
                tenant_field => format!("tenant_{}", idx)
            ))?;
        }
        
        writer.commit()?;
    }
    
    let elapsed = start.elapsed();
    let after_rss = get_rss_mb();
    
    println!("Total docs indexed: {}", num_indices * docs_per_index);
    println!("Time: {:.2}s ({:.0} docs/sec)", 
        elapsed.as_secs_f64(),
        (num_indices * docs_per_index) as f64 / elapsed.as_secs_f64()
    );
    
    if let (Some(before), Some(after)) = (base_rss, after_rss) {
        let overhead = (after - before) / num_indices as f64;
        println!("RSS before: {:.1} MB", before);
        println!("RSS after: {:.1} MB", after);
        println!("Per-index overhead: {:.1} MB", overhead);
    }
    
    Ok(())
}

fn test_3_concurrent_readers() -> tantivy::Result<()> {
    println!("\n=== Test 3: Concurrent Reader Overhead ===");
    let base_rss = get_rss_mb();
    
    let num_indices = 20;
    let mut readers: Vec<IndexReader> = Vec::new();
    
    // Open all indices from Test 2
    for idx in 0..num_indices {
        let index_path = PathBuf::from(format!("/tmp/tantivy_test_realistic_{}", idx));
        let _schema = create_schema();
        let index = Index::open_in_dir(&index_path)?;
        let reader = index.reader()?;
        readers.push(reader);
    }
    
    let after_open_rss = get_rss_mb();
    
    // Run queries across all readers
    let start = Instant::now();
    let queries_per_index = 100;
    let mut total_results = 0;
    
    for reader in &readers {
        let searcher = reader.searcher();
        let schema = searcher.schema();
        let title_field = schema.get_field("title").unwrap();
        
        for q in 0..queries_per_index {
            let query_parser = tantivy::query::QueryParser::for_index(
                searcher.index(), 
                vec![title_field]
            );
            let query = query_parser.parse_query(&format!("Product {}", q * 100))?;
            let top_docs = searcher.search(&query, &tantivy::collector::TopDocs::with_limit(10))?;
            total_results += top_docs.len();
        }
    }
    
    let elapsed = start.elapsed();
    let after_query_rss = get_rss_mb();
    
    println!("Readers opened: {}", num_indices);
    println!("Queries executed: {}", num_indices * queries_per_index);
    println!("Avg query time: {:.2}ms", elapsed.as_millis() as f64 / (num_indices * queries_per_index) as f64);
    println!("Total results: {}", total_results);
    
    if let Some(before) = base_rss {
        if let Some(after_open) = after_open_rss {
            println!("RSS before: {:.1} MB", before);
            println!("RSS after opening readers: {:.1} MB", after_open);
            println!("Per-reader overhead: {:.1} MB", (after_open - before) / num_indices as f64);
        }
        if let Some(after_query) = after_query_rss {
            println!("RSS after queries: {:.1} MB", after_query);
        }
    }
    
    Ok(())
}

fn main() -> tantivy::Result<()> {
    println!("Tantivy Multi-Tenancy Viability Test");
    println!("=====================================");
    
    test_1_empty_index_overhead()?;
    test_2_realistic_overhead()?;
    test_3_concurrent_readers()?;
    
    println!("\n=== Disk Usage ===");
    let empty_size = std::process::Command::new("du")
        .args(&["-sh", "/tmp/tantivy_test_empty_0"])
        .output();
    let realistic_size = std::process::Command::new("du")
        .args(&["-sh", "/tmp/tantivy_test_realistic_0"])
        .output();
    
    if let Ok(out) = empty_size {
        println!("Empty index: {}", String::from_utf8_lossy(&out.stdout).trim());
    }
    if let Ok(out) = realistic_size {
        println!("10K-doc index: {}", String::from_utf8_lossy(&out.stdout).trim());
    }
    
    println!("\n✅ Tests complete");
    
    Ok(())
}