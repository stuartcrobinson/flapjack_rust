use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy};
use tempfile::TempDir;

fn get_rss_mb() -> f64 {
    let pid = std::process::id();
    let status_path = format!("/proc/{}/status", pid);
    
    match fs::read_to_string(&status_path) {
        Ok(contents) => {
            for line in contents.lines() {
                if line.starts_with("VmRSS:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<f64>() {
                            return kb / 1024.0;
                        }
                    }
                }
            }
            0.0
        }
        Err(_) => 0.0,
    }
}

fn create_tenant_index(base_path: &Path, tenant_id: u32, doc_count: usize) -> PathBuf {
    let tenant_path = base_path.join(format!("tenant_{}", tenant_id));
    fs::create_dir_all(&tenant_path).unwrap();

    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    schema_builder.add_u64_field("timestamp", INDEXED | STORED);
    schema_builder.add_u64_field("price", FAST);
    let schema = schema_builder.build();

    let index = Index::create_in_dir(&tenant_path, schema.clone()).unwrap();
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    for i in 0..doc_count {
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let timestamp = schema.get_field("timestamp").unwrap();
        let price = schema.get_field("price").unwrap();

        writer
            .add_document(doc!(
                title => format!("Document {} tenant {}", i, tenant_id),
                body => format!("searchable content document {} tenant {} extra words for realistic size", i, tenant_id),
                timestamp => (1700000000 + i) as u64,
                price => (i % 1000) as u64,
            ))
            .unwrap();
    }

    writer.commit().unwrap();
    tenant_path
}

struct TenantState {
    _index: Index,
    reader: IndexReader,
}

fn test_open_only(tenant_paths: &[PathBuf]) -> Result<f64, String> {
    println!("\n=== TEST A: Open Indexes Only (No Queries) ===");
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    let mut indexes = Vec::new();
    for (i, path) in tenant_paths.iter().enumerate() {
        let index = Index::open_in_dir(path)
            .map_err(|e| format!("Failed to open index {}: {}", i, e))?;
        indexes.push(index);
    }

    let after_open_rss = get_rss_mb();
    let open_cost = after_open_rss - baseline_rss;

    println!("After opening {} indexes:", indexes.len());
    println!("  RSS: {:.2} MB (+{:.2} MB)", after_open_rss, open_cost);
    println!("  Per-index: {:.3} MB", open_cost / indexes.len() as f64);

    Ok(open_cost)
}

fn test_open_with_readers(tenant_paths: &[PathBuf]) -> Result<f64, String> {
    println!("\n=== TEST B: Open + Create Readers (No Queries) ===");
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    let mut states = Vec::new();
    for (i, path) in tenant_paths.iter().enumerate() {
        let index = Index::open_in_dir(path)
            .map_err(|e| format!("Failed to open index {}: {}", i, e))?;
        
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| format!("Failed to create reader {}: {}", i, e))?;
        
        states.push(TenantState { _index: index, reader });
        
        if (i + 1) % 50 == 0 {
            let current_rss = get_rss_mb();
            println!(
                "  Created {} readers - RSS: {:.2} MB (+{:.2} MB)",
                i + 1,
                current_rss,
                current_rss - baseline_rss
            );
        }
    }

    let after_readers_rss = get_rss_mb();
    let reader_cost = after_readers_rss - baseline_rss;

    println!("\nAfter creating {} readers:", states.len());
    println!("  RSS: {:.2} MB (+{:.2} MB)", after_readers_rss, reader_cost);
    println!("  Per-reader: {:.3} MB", reader_cost / states.len() as f64);

    Ok(reader_cost)
}

fn test_active_queries(tenant_paths: &[PathBuf]) -> Result<f64, String> {
    println!("\n=== TEST C: Open + Readers + Active Queries ===");
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    let mut states = Vec::new();
    for (i, path) in tenant_paths.iter().enumerate() {
        let index = Index::open_in_dir(path)
            .map_err(|e| format!("Failed to open index {}: {}", i, e))?;
        
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| format!("Failed to create reader {}: {}", i, e))?;
        
        states.push(TenantState { _index: index, reader });
    }

    let after_readers_rss = get_rss_mb();
    println!("After creating readers: {:.2} MB (+{:.2} MB)", 
        after_readers_rss, after_readers_rss - baseline_rss);

    // Now run queries to fault in working set
    println!("\nExecuting queries to fault in working sets...");
    let query_start = Instant::now();
    
    for (i, state) in states.iter().enumerate() {
        let searcher = state.reader.searcher();
        let schema = searcher.schema();
        let body_field = schema.get_field("body").unwrap();
        
        let query_parser = QueryParser::for_index(state.reader.searcher().index(), vec![body_field]);
        let query = query_parser.parse_query("searchable content document")
            .map_err(|e| format!("Query parse error: {}", e))?;
        
        // Execute query - this faults in pages
        let _results = searcher.search(&query, &TopDocs::with_limit(10))
            .map_err(|e| format!("Search error on tenant {}: {}", i, e))?;
        
        if (i + 1) % 50 == 0 {
            let current_rss = get_rss_mb();
            println!(
                "  Queried {} tenants - RSS: {:.2} MB (+{:.2} MB)",
                i + 1,
                current_rss,
                current_rss - baseline_rss
            );
        }
    }

    let query_duration = query_start.elapsed();
    let after_queries_rss = get_rss_mb();
    let query_working_set = after_queries_rss - baseline_rss;

    println!("\nAfter querying all {} tenants:", states.len());
    println!("  RSS: {:.2} MB (+{:.2} MB)", after_queries_rss, query_working_set);
    println!("  Per-tenant working set: {:.3} MB", query_working_set / states.len() as f64);
    println!("  Query execution time: {:?}", query_duration);
    println!("  Avg query time: {:?}", query_duration / states.len() as u32);

    Ok(query_working_set)
}

fn test_lru_query_pattern(tenant_paths: &[PathBuf], cache_size: usize, total_queries: usize) -> Result<f64, String> {
    println!("\n=== TEST D: LRU Query Pattern (cache_size={}, {} queries) ===", cache_size, total_queries);
    
    let baseline_rss = get_rss_mb();
    println!("Baseline RSS: {:.2} MB", baseline_rss);

    // Simulate LRU: keep only cache_size readers active
    let mut cache: Vec<Option<TenantState>> = (0..cache_size).map(|_| None).collect();
    let mut cache_idx = 0;
    let mut max_rss = baseline_rss;

    println!("\nSimulating Zipf query distribution (80/20)...");
    let query_start = Instant::now();
    
    // Zipf-like pattern: 80% of queries hit 20% of tenants
    let hot_tenant_count = tenant_paths.len() / 5; // 20% hot
    
    for i in 0..total_queries {
        // 80% of time, query a hot tenant
        let tenant_idx = if i % 5 < 4 {
            i % hot_tenant_count // Hot tenants
        } else {
            hot_tenant_count + (i % (tenant_paths.len() - hot_tenant_count)) // Cold tenants
        };
        
        let path = &tenant_paths[tenant_idx];
        
        // Evict oldest from cache
        cache[cache_idx] = None;
        
        // Load tenant state
        let index = Index::open_in_dir(path)
            .map_err(|e| format!("Failed to open index: {}", e))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| format!("Failed to create reader: {}", e))?;
        
        let state = TenantState { _index: index, reader };
        
        // Execute query
        let searcher = state.reader.searcher();
        let schema = searcher.schema();
        let body_field = schema.get_field("body").unwrap();
        let query_parser = QueryParser::for_index(state.reader.searcher().index(), vec![body_field]);
        let query = query_parser.parse_query("searchable content")
            .map_err(|e| format!("Query parse error: {}", e))?;
        let _results = searcher.search(&query, &TopDocs::with_limit(10))
            .map_err(|e| format!("Search error: {}", e))?;
        
        // Store in cache
        cache[cache_idx] = Some(state);
        cache_idx = (cache_idx + 1) % cache_size;
        
        let current_rss = get_rss_mb();
        if current_rss > max_rss {
            max_rss = current_rss;
        }
        
        if (i + 1) % 200 == 0 {
            println!(
                "  {} queries - RSS: {:.2} MB (+{:.2} MB, peak: +{:.2} MB)",
                i + 1,
                current_rss,
                current_rss - baseline_rss,
                max_rss - baseline_rss
            );
        }
    }

    let query_duration = query_start.elapsed();
    let final_rss = get_rss_mb();
    let peak_working_set = max_rss - baseline_rss;

    println!("\nAfter {} queries with LRU cache:", total_queries);
    println!("  Peak RSS: {:.2} MB (+{:.2} MB)", max_rss, peak_working_set);
    println!("  Final RSS: {:.2} MB", final_rss);
    println!("  Expected for cache_size={}: ~{:.2} MB", cache_size, cache_size as f64 * 2.38);
    println!("  Total query time: {:?}", query_duration);
    println!("  Avg query time: {:?}", query_duration / total_queries as u32);

    Ok(peak_working_set)
}

fn main() {
    println!("==============================================");
    println!("Query Working Set Test");
    println!("==============================================");
    println!("\nHypothesis: Index::open() is free, but active queries consume working set");
    println!("\nTest plan:");
    println!("  A) Open indexes only (no readers)");
    println!("  B) Open + create readers (no queries)");
    println!("  C) Open + readers + execute queries");
    println!("  D) LRU query pattern (Zipf distribution)");

    let initial_rss = get_rss_mb();
    println!("\nInitial RSS: {:.2} MB", initial_rss);
    
    if initial_rss < 1.0 {
        eprintln!("ERROR: RSS measurement broken");
        return;
    }

    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    println!("\n--- Creating tenant indexes ---");
    let tenant_count = 200;
    let docs_per_tenant = 1_000;
    
    let mut tenant_paths = Vec::new();
    let create_start = Instant::now();
    
    for i in 0..tenant_count {
        if i % 40 == 0 {
            println!("Creating tenant {}...", i);
        }
        let path = create_tenant_index(base_path, i, docs_per_tenant);
        tenant_paths.push(path);
    }
    
    println!("✅ Created {} tenants in {:?}", tenant_count, create_start.elapsed());

    let test_a = test_open_only(&tenant_paths);
    let test_b = test_open_with_readers(&tenant_paths);
    let test_c = test_active_queries(&tenant_paths);
    let test_d = test_lru_query_pattern(&tenant_paths, 120, 1000);

    println!("\n\n==============================================");
    println!("RESULTS SUMMARY");
    println!("==============================================");

    if let Ok(cost) = test_a {
        println!("\n📊 Test A - Open Only: {:.2} MB ({:.3} MB/index)", 
            cost, cost / tenant_count as f64);
    }

    if let Ok(cost) = test_b {
        println!("📊 Test B - Open + Readers: {:.2} MB ({:.3} MB/reader)", 
            cost, cost / tenant_count as f64);
    }

    if let Ok(cost) = test_c {
        println!("📊 Test C - Active Queries: {:.2} MB ({:.3} MB/tenant)", 
            cost, cost / tenant_count as f64);
        println!("   ^^^ This is your actual per-tenant working set");
    }

    if let Ok(cost) = test_d {
        println!("📊 Test D - LRU Pattern (120 cache): {:.2} MB", cost);
        println!("   Expected: ~{:.2} MB", 120.0 * 2.38);
        
        if cost < 400.0 {
            println!("   ✅ LRU cache bounds memory as designed");
        } else {
            println!("   ⚠️  Higher than expected - check assumptions");
        }
    }

    println!("\n==============================================");
    println!("CONCLUSION");
    println!("==============================================");

    match (test_a, test_c, test_d) {
        (Ok(open_cost), Ok(query_cost), Ok(lru_cost)) => {
            println!("\nKey findings:");
            println!("  1. Index::open() cost: {:.2} MB", open_cost);
            println!("  2. Active query cost: {:.2} MB", query_cost);
            println!("  3. LRU (120 tenants): {:.2} MB", lru_cost);
            
            if open_cost < 10.0 {
                println!("\n✅ Confirms: Index::open() is essentially free");
            }
            
            if query_cost > open_cost * 10.0 {
                println!("✅ Confirms: Query working set dominates memory");
            }
            
            if query_cost > lru_cost * 1.5 {
                println!("✅ Confirms: LRU cache reduces memory by {:.1}%", 
                    ((query_cost - lru_cost) / query_cost) * 100.0);
                println!("\nRecommendation: LRU cache is ESSENTIAL for bounding query working set");
            } else {
                println!("\n⚠️  LRU cache provides minimal benefit");
                println!("Recommendation: May not need cache, but consider fd limits");
            }
        }
        _ => println!("\n❌ Tests failed - review errors above"),
    }

    println!("\n==============================================\n");
}


// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin query_working_set_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 53.60s
//      Running `target/release/query_working_set_test`
// ==============================================
// Query Working Set Test
// ==============================================

// Hypothesis: Index::open() is free, but active queries consume working set

// Test plan:
//   A) Open indexes only (no readers)
//   B) Open + create readers (no queries)
//   C) Open + readers + execute queries
//   D) LRU query pattern (Zipf distribution)

// Initial RSS: 3.42 MB

// --- Creating tenant indexes ---
// Creating tenant 0...
// Creating tenant 40...
// Creating tenant 80...
// Creating tenant 120...
// Creating tenant 160...
// ✅ Created 200 tenants in 18.647564433s

// === TEST A: Open Indexes Only (No Queries) ===
// Baseline RSS: 99.16 MB
// After opening 200 indexes:
//   RSS: 99.16 MB (+0.00 MB)
//   Per-index: 0.000 MB

// === TEST B: Open + Create Readers (No Queries) ===
// Baseline RSS: 99.16 MB
//   Created 50 readers - RSS: 102.98 MB (+3.82 MB)
//   Created 100 readers - RSS: 106.91 MB (+7.75 MB)
//   Created 150 readers - RSS: 110.72 MB (+11.56 MB)
//   Created 200 readers - RSS: 114.66 MB (+15.50 MB)

// After creating 200 readers:
//   RSS: 114.66 MB (+15.50 MB)
//   Per-reader: 0.077 MB

// === TEST C: Open + Readers + Active Queries ===
// Baseline RSS: 99.21 MB
// After creating readers: 114.60 MB (+15.39 MB)

// Executing queries to fault in working sets...
//   Queried 50 tenants - RSS: 114.88 MB (+15.66 MB)
//   Queried 100 tenants - RSS: 115.00 MB (+15.79 MB)
//   Queried 150 tenants - RSS: 115.00 MB (+15.79 MB)
//   Queried 200 tenants - RSS: 115.12 MB (+15.91 MB)

// After querying all 200 tenants:
//   RSS: 115.12 MB (+15.91 MB)
//   Per-tenant working set: 0.080 MB
//   Query execution time: 6.588701ms
//   Avg query time: 32.943µs

// === TEST D: LRU Query Pattern (cache_size=120, 1000 queries) ===
// Baseline RSS: 99.33 MB

// Simulating Zipf query distribution (80/20)...
//   200 queries - RSS: 108.88 MB (+9.54 MB, peak: +9.54 MB)
//   400 queries - RSS: 108.88 MB (+9.54 MB, peak: +9.54 MB)
//   600 queries - RSS: 108.88 MB (+9.54 MB, peak: +9.54 MB)
//   800 queries - RSS: 108.88 MB (+9.54 MB, peak: +9.54 MB)
//   1000 queries - RSS: 108.88 MB (+9.54 MB, peak: +9.54 MB)

// After 1000 queries with LRU cache:
//   Peak RSS: 108.88 MB (+9.54 MB)
//   Final RSS: 108.88 MB
//   Expected for cache_size=120: ~285.60 MB
//   Total query time: 376.934387ms
//   Avg query time: 376.934µs


// ==============================================
// RESULTS SUMMARY
// ==============================================

// 📊 Test A - Open Only: 0.00 MB (0.000 MB/index)
// 📊 Test B - Open + Readers: 15.50 MB (0.077 MB/reader)
// 📊 Test C - Active Queries: 15.91 MB (0.080 MB/tenant)
//    ^^^ This is your actual per-tenant working set
// 📊 Test D - LRU Pattern (120 cache): 9.54 MB
//    Expected: ~285.60 MB
//    ✅ LRU cache bounds memory as designed

// ==============================================
// CONCLUSION
// ==============================================

// Key findings:
//   1. Index::open() cost: 0.00 MB
//   2. Active query cost: 15.91 MB
//   3. LRU (120 tenants): 9.54 MB

// ✅ Confirms: Index::open() is essentially free
// ✅ Confirms: Query working set dominates memory
// ✅ Confirms: LRU cache reduces memory by 40.0%

// Recommendation: LRU cache is ESSENTIAL for bounding query working set

// ==============================================

// ubuntu@ip-172-31-23-154:~/flapjack_rust$