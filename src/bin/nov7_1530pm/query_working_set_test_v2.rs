// ## Source of 2.38 MB Discrepancy Found

// **Test: `realistic_density_test` (nov6_1530pm/FINAL_RESULTS.md)**
// - 150 tenants × 50K docs = 7.5M docs total
// - RSS: 360 MB steady-state
// - **Per-tenant: 2.38 MB**

// **Your new test: `query_working_set_test`**
// - 200 tenants × 1K docs = 200K docs total  
// - RSS: 15.91 MB total
// - **Per-tenant: 0.080 MB**

// ## The 30x Difference: Document Count

// **Scaling analysis:**

// ```
// Old test: 50K docs → 2.38 MB
// New test: 1K docs → 0.08 MB

// Ratio: 50K/1K = 50x docs
//        2.38/0.08 = 29.75x memory

// Non-linear scaling: √(doc_count) growth pattern
// ```

// **Hypothesis: Fast fields dominate at scale**

// Your schema has `price` field with FAST attribute. Memory cost:
// - 1K docs × u64 = 8 KB (negligible)
// - 50K docs × u64 = 400 KB per tenant
// - Plus codec overhead, column stride, etc.

// At 150 tenants × 50K docs, if fast fields are 400 KB each:
// - 150 × 400 KB = 60 MB just for one field
// - Multiple FAST fields → explains the bulk

// ## Validation: Check Memory Density Test

// From `tantivy_memory_density_test` results:
// - 50 tenants × 10K docs
// - RSS: 204.96 MB
// - **Per-tenant: 4.10 MB**

// ```
// 10K docs → 4.10 MB
// 50K docs → 2.38 MB (contradicts!)
// ```

// **This is backwards.** Either:
// 1. Different schema (fewer FAST fields in realistic test)
// 2. Measurement error (baseline RSS subtraction issue)
// 3. Mmap hasn't fully faulted in yet

// ## Critical Issue: Your Tests Use Different Schemas

// **Filter selectivity test** (10K docs):
// ```rust
// price: FAST | STORED
// id: FAST | STORED
// title: TEXT | STORED
// ```

// **Your working set test** (1K docs):
// ```rust
// price: FAST (only)
// timestamp: INDEXED | STORED
// title: TEXT | STORED
// body: TEXT
// ```

// **If realistic_density_test used 5+ FAST fields** (mentioned in Test 3), that explains it.

// ## The Real Per-Tenant Cost

// Need to test with **realistic schema at realistic scale**:

// ```rust
// // What realistic_density_test likely used:
// 50K docs per tenant
// 5 FAST fields (sortable columns)
// 2-3 TEXT fields
// Query working set after Zipf distribution queries
// ```

// Your 0.080 MB measurement is for:
// - Toy schema (1 FAST field)
// - Tiny corpus (1K docs)
// - Cold query (first access)

// ## Proposed Test

// Rerun `query_working_set_test` with:
// 1. **50K docs/tenant** (not 1K)
// 2. **5 FAST fields** (price, timestamp, category_id, user_id, rating)
// 3. **Measure after 10K queries** (fully warmed up)

// Expected result: 2-3 MB/tenant, validating original 2.38 MB.

// ## If 2.38 MB Holds at 50K Docs

// **Revised capacity:**
// - 4 GB / 2.38 MB = 1680 tenants theoretical
// - 60% utilization = 1008 tenants safe
// - **Your 600 target is conservative, can push to 800**

// ## If It's Still 0.080 MB/tenant

// Something is fundamentally wrong with realistic_density_test measurement. Either:
// - Mmap pages not faulted in (RSS doesn't reflect reality)
// - Baseline subtraction error (miscounting)
// - Schema mismatch (realistic test had bloated fields)

// **Action: Find the actual schema/setup for realistic_density_test code.** The 2.38 MB number is in your constraints doc for a reason - need to validate or refute it definitively with matching test conditions.

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
    schema_builder.add_u64_field("timestamp", FAST | STORED);
    schema_builder.add_u64_field("price", FAST | STORED);
    schema_builder.add_u64_field("category_id", FAST | STORED);
    schema_builder.add_u64_field("user_id", FAST | STORED);
    schema_builder.add_u64_field("rating", FAST | STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_dir(&tenant_path, schema.clone()).unwrap();
    let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

    let title = schema.get_field("title").unwrap();
    let body = schema.get_field("body").unwrap();
    let timestamp = schema.get_field("timestamp").unwrap();
    let price = schema.get_field("price").unwrap();
    let category_id = schema.get_field("category_id").unwrap();
    let user_id = schema.get_field("user_id").unwrap();
    let rating = schema.get_field("rating").unwrap();

    for i in 0..doc_count {
        writer
            .add_document(doc!(
                title => format!("Document {} tenant {}", i, tenant_id),
                body => format!("searchable content document {} tenant {} extra words for realistic size", i, tenant_id),
                timestamp => (1700000000 + i) as u64,
                price => (i % 1000) as u64,
                category_id => (i % 50) as u64,
                user_id => (i % 10000) as u64,
                rating => (i % 5) as u64,
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
    let tenant_count = 150;
    let docs_per_tenant = 50_000;
    
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
    let test_d = test_lru_query_pattern(&tenant_paths, 120, 5000);

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

// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin query_working_set_test_v2
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 54.70s
//      Running `target/release/query_working_set_test_v2`
// ==============================================
// Query Working Set Test
// ==============================================

// Hypothesis: Index::open() is free, but active queries consume working set

// Test plan:
//   A) Open indexes only (no readers)
//   B) Open + create readers (no queries)
//   C) Open + readers + execute queries
//   D) LRU query pattern (Zipf distribution)

// Initial RSS: 3.39 MB

// --- Creating tenant indexes ---
// Creating tenant 0...
// Creating tenant 40...
// Creating tenant 80...
// Creating tenant 120...
// ✅ Created 150 tenants in 50.325314167s

// === TEST A: Open Indexes Only (No Queries) ===
// Baseline RSS: 136.21 MB
// After opening 150 indexes:
//   RSS: 136.21 MB (+0.00 MB)
//   Per-index: 0.000 MB

// === TEST B: Open + Create Readers (No Queries) ===
// Baseline RSS: 136.21 MB
//   Created 50 readers - RSS: 155.61 MB (+19.39 MB)
//   Created 100 readers - RSS: 175.23 MB (+39.01 MB)
//   Created 150 readers - RSS: 195.48 MB (+59.27 MB)

// After creating 150 readers:
//   RSS: 195.48 MB (+59.27 MB)
//   Per-reader: 0.395 MB

// === TEST C: Open + Readers + Active Queries ===
// Baseline RSS: 136.18 MB
// After creating readers: 195.37 MB (+59.19 MB)

// Executing queries to fault in working sets...
//   Queried 50 tenants - RSS: 215.13 MB (+78.95 MB)
//   Queried 100 tenants - RSS: 233.98 MB (+97.80 MB)
//   Queried 150 tenants - RSS: 252.60 MB (+116.41 MB)

// After querying all 150 tenants:
//   RSS: 252.60 MB (+116.41 MB)
//   Per-tenant working set: 0.776 MB
//   Query execution time: 19.345841ms
//   Avg query time: 128.972µs

// === TEST D: LRU Query Pattern (cache_size=120, 5000 queries) ===
// Baseline RSS: 136.28 MB

// Simulating Zipf query distribution (80/20)...
//   200 queries - RSS: 226.10 MB (+89.82 MB, peak: +89.93 MB)
//   400 queries - RSS: 226.33 MB (+90.05 MB, peak: +90.05 MB)
//   600 queries - RSS: 226.13 MB (+89.86 MB, peak: +90.05 MB)
//   800 queries - RSS: 226.23 MB (+89.95 MB, peak: +90.05 MB)
//   1000 queries - RSS: 226.17 MB (+89.89 MB, peak: +90.05 MB)
//   1200 queries - RSS: 226.11 MB (+89.83 MB, peak: +90.05 MB)
//   1400 queries - RSS: 226.07 MB (+89.79 MB, peak: +90.05 MB)
//   1600 queries - RSS: 226.20 MB (+89.92 MB, peak: +90.05 MB)
//   1800 queries - RSS: 226.23 MB (+89.96 MB, peak: +90.05 MB)
//   2000 queries - RSS: 226.13 MB (+89.85 MB, peak: +90.05 MB)
//   2200 queries - RSS: 226.14 MB (+89.87 MB, peak: +90.05 MB)
//   2400 queries - RSS: 226.25 MB (+89.97 MB, peak: +90.05 MB)
//   2600 queries - RSS: 226.16 MB (+89.89 MB, peak: +90.05 MB)
//   2800 queries - RSS: 226.17 MB (+89.89 MB, peak: +90.05 MB)
//   3000 queries - RSS: 226.11 MB (+89.83 MB, peak: +90.05 MB)
//   3200 queries - RSS: 226.13 MB (+89.85 MB, peak: +90.05 MB)
//   3400 queries - RSS: 226.27 MB (+89.99 MB, peak: +90.05 MB)
//   3600 queries - RSS: 226.17 MB (+89.89 MB, peak: +90.05 MB)
//   3800 queries - RSS: 226.10 MB (+89.82 MB, peak: +90.05 MB)
//   4000 queries - RSS: 226.12 MB (+89.84 MB, peak: +90.05 MB)
//   4200 queries - RSS: 226.17 MB (+89.89 MB, peak: +90.05 MB)
//   4400 queries - RSS: 226.13 MB (+89.85 MB, peak: +90.05 MB)
//   4600 queries - RSS: 226.24 MB (+89.96 MB, peak: +90.05 MB)
//   4800 queries - RSS: 226.17 MB (+89.89 MB, peak: +90.08 MB)
//   5000 queries - RSS: 226.21 MB (+89.93 MB, peak: +90.08 MB)

// After 5000 queries with LRU cache:
//   Peak RSS: 226.36 MB (+90.08 MB)
//   Final RSS: 226.21 MB
//   Expected for cache_size=120: ~285.60 MB
//   Total query time: 2.565719911s
//   Avg query time: 513.143µs


// ==============================================
// RESULTS SUMMARY
// ==============================================

// 📊 Test A - Open Only: 0.00 MB (0.000 MB/index)
// 📊 Test B - Open + Readers: 59.27 MB (0.395 MB/reader)
// 📊 Test C - Active Queries: 116.41 MB (0.776 MB/tenant)
//    ^^^ This is your actual per-tenant working set
// 📊 Test D - LRU Pattern (120 cache): 90.08 MB
//    Expected: ~285.60 MB
//    ✅ LRU cache bounds memory as designed

// ==============================================
// CONCLUSION
// ==============================================

// Key findings:
//   1. Index::open() cost: 0.00 MB
//   2. Active query cost: 116.41 MB
//   3. LRU (120 tenants): 90.08 MB

// ✅ Confirms: Index::open() is essentially free
// ✅ Confirms: Query working set dominates memory

// ⚠️  LRU cache provides minimal benefit
// Recommendation: May not need cache, but consider fd limits

// ==============================================

// ubuntu@ip-172-31-23-154:~/flapjack_rust$