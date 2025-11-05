// src/bin/write_latency_test.rs
use std::path::PathBuf;
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, TantivyDocument};
use std::time::Instant;
use std::thread;
use std::sync::Arc;

fn get_rss_mb() -> f64 {
    #[cfg(target_os = "macos")]
    {
        use mach2::mach_types::task_info_t;
        use mach2::task::{task_info, TASK_VM_INFO};
        use mach2::task_info::{task_vm_info, TASK_VM_INFO_COUNT};
        use mach2::traps::mach_task_self;
        use std::mem;

        unsafe {
            let mut info: task_vm_info = mem::zeroed();
            let mut count = TASK_VM_INFO_COUNT;
            let result = task_info(
                mach_task_self(),
                TASK_VM_INFO,
                &mut info as *mut task_vm_info as task_info_t,
                &mut count,
            );
            if result == 0 {
                return info.phys_footprint as f64 / 1024.0 / 1024.0;
            }
        }
    }
    
    #[cfg(not(target_os = "macos"))]
    {
        // Linux fallback - read from /proc/self/status
        if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(kb_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = kb_str.parse::<f64>() {
                            return kb / 1024.0;
                        }
                    }
                }
            }
        }
    }
    
    0.0
}

fn create_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("body", TEXT);
    schema_builder.add_i64_field("price", INDEXED | STORED | FAST);
    schema_builder.add_text_field("sku", STRING | STORED);
    schema_builder.build()
}

fn main() -> tantivy::Result<()> {
    println!("=== Tantivy Real-Time Write Test ===\n");
    
    // Test 1: Single-doc write latency (Algolia pattern)
    {
        println!("Test 1: Single-doc write latency (real-time pattern)");
        println!("Simulating: User adds product, needs instant searchability\n");
        
        let index_path = PathBuf::from("/tmp/tantivy_realtime_test");
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let schema = create_schema();
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000)?;
        
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let price = schema.get_field("price").unwrap();
        let sku = schema.get_field("sku").unwrap();
        
        // Warm up
        for i in 0..100 {
            writer.add_document(doc!(
                title => format!("Product {}", i),
                body => "test product description",
                price => 1000i64,
                sku => format!("SKU{}", i)
            ))?;
        }
        writer.commit()?;
        
        // Measure single-doc write-commit cycle
        let mut latencies = Vec::new();
        
        for i in 0..100 {
            let start = Instant::now();
            
            writer.add_document(doc!(
                title => format!("Product {}", i + 1000),
                body => "new product just added",
                price => 2000i64,
                sku => format!("SKU{}", i + 1000)
            ))?;
            
            writer.commit()?;
            
            let elapsed = start.elapsed().as_millis();
            latencies.push(elapsed);
        }
        
        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() * 95) / 100];
        let p99 = latencies[(latencies.len() * 99) / 100];
        
        println!("  Single-doc add+commit latency:");
        println!("    P50: {}ms", p50);
        println!("    P95: {}ms", p95);
        println!("    P99: {}ms", p99);
        
        if p99 > 1000 {
            println!("  ⚠️  P99 >1s: Too slow for real-time UX");
        } else if p99 > 500 {
            println!("  ⚠️  P99 >500ms: Marginal for real-time");
        } else {
            println!("  ✓ Acceptable for real-time writes");
        }
        
        // Check segment proliferation
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let num_segments = searcher.segment_readers().len();
        
        println!("  Segments after 100 commits: {}", num_segments);
        if num_segments > 20 {
            println!("  ⚠️  High segment count: Merge pressure building");
        }
        println!();
    }
    
    // Test 2: Concurrent writes across tenants
    {
        println!("Test 2: Concurrent multi-tenant writes");
        println!("Simulating: 10 tenants each writing simultaneously\n");
        
        let num_tenants = 10;
        let writes_per_tenant = 50;
        
        let base_rss = get_rss_mb();
        
        let handles: Vec<_> = (0..num_tenants)
            .map(|tenant_id| {
                thread::spawn(move || {
                    let index_path = PathBuf::from(format!("/tmp/tantivy_tenant_{}", tenant_id));
                    let _ = std::fs::remove_dir_all(&index_path);
                    std::fs::create_dir_all(&index_path).unwrap();
                    
                    let schema = create_schema();
                    let index = Index::create_in_dir(&index_path, schema.clone()).unwrap();
                    let mut writer: IndexWriter<TantivyDocument> = index.writer(50_000_000).unwrap();
                    
                    let title = schema.get_field("title").unwrap();
                    let body = schema.get_field("body").unwrap();
                    let price = schema.get_field("price").unwrap();
                    let sku = schema.get_field("sku").unwrap();
                    
                    let mut latencies = Vec::new();
                    
                    for i in 0..writes_per_tenant {
                        let start = Instant::now();
                        
                        writer.add_document(doc!(
                            title => format!("Tenant {} Product {}", tenant_id, i),
                            body => "concurrent write test",
                            price => 1500i64,
                            sku => format!("T{}SKU{}", tenant_id, i)
                        )).unwrap();
                        
                        writer.commit().unwrap();
                        
                        latencies.push(start.elapsed().as_millis());
                    }
                    
                    latencies.sort();
                    (tenant_id, latencies)
                })
            })
            .collect();
        
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        
        // Aggregate stats
        let mut all_latencies: Vec<u128> = Vec::new();
        for (_tenant_id, latencies) in &results {
            all_latencies.extend(latencies);
        }
        
        all_latencies.sort();
        let p50 = all_latencies[all_latencies.len() / 2];
        let p95 = all_latencies[(all_latencies.len() * 95) / 100];
        let p99 = all_latencies[(all_latencies.len() * 99) / 100];
        
        println!("  Concurrent write latency ({} tenants):", num_tenants);
        println!("    P50: {}ms", p50);
        println!("    P95: {}ms", p95);
        println!("    P99: {}ms", p99);
        
        let after_rss = get_rss_mb();
        if let (Some(before), Some(after)) = (base_rss, after_rss) {
            println!("  RSS: {:.1} MB → {:.1} MB", before, after);
            println!("  Per-tenant overhead: {:.1} MB", (after - before) / num_tenants as f64);
        }
        println!();
    }
    
    // Test 3: Search visibility delay
    {
        println!("Test 3: Write-to-search visibility");
        println!("Measuring: How fast can users search newly added docs?\n");
        
        let index_path = PathBuf::from("/tmp/tantivy_visibility_test");
        let _ = std::fs::remove_dir_all(&index_path);
        std::fs::create_dir_all(&index_path)?;
        
        let schema = create_schema();
        let index = Index::create_in_dir(&index_path, schema.clone())?;
        let index_arc = Arc::new(index);
        
        let mut writer: IndexWriter<TantivyDocument> = index_arc.writer(50_000_000)?;
        
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let price = schema.get_field("price").unwrap();
        let sku = schema.get_field("sku").unwrap();
        
        // Add initial docs
        for i in 0..1000 {
            writer.add_document(doc!(
                title => format!("Initial Product {}", i),
                body => "baseline document",
                price => 1000i64,
                sku => format!("INIT{}", i)
            ))?;
        }
        writer.commit()?;
        
        let reader = index_arc
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        
        // Test visibility delay
        let unique_sku = "TESTSKU_UNIQUE_12345";
        let write_start = Instant::now();
        
        writer.add_document(doc!(
            title => "Visibility Test Product",
            body => "testing search visibility",
            price => 9999i64,
            sku => unique_sku
        ))?;
        
        writer.commit()?;
        let commit_done = write_start.elapsed();
        
        // Poll for visibility
        let mut visible = false;
        let mut check_count = 0;
        let visibility_start = Instant::now();
        
        while visibility_start.elapsed().as_secs() < 5 && !visible {
            check_count += 1;
            reader.reload()?;
            let searcher = reader.searcher();
            
            let query_parser = tantivy::query::QueryParser::for_index(
                &index_arc,
                vec![schema.get_field("sku").unwrap()]
            );
            let query = query_parser.parse_query(unique_sku)?;
            let count = searcher.search(&query, &tantivy::collector::Count)?;
            
            if count > 0 {
                visible = true;
            } else {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        
        let visibility_delay = visibility_start.elapsed();
        
        println!("  Commit latency: {}ms", commit_done.as_millis());
        println!("  Visibility delay: {}ms ({} reload attempts)", 
            visibility_delay.as_millis(), check_count);
        
        if visibility_delay.as_millis() > 1000 {
            println!("  ⚠️  >1s visibility: Not competitive with Algolia");
        } else if visibility_delay.as_millis() > 500 {
            println!("  ⚠️  >500ms visibility: Marginal vs Algolia");
        } else {
            println!("  ✓ Sub-500ms visibility: Competitive");
        }
    }
    
    println!("\n=== Verdict ===");
    println!("Tantivy is viable IF:");
    println!("  - Single-doc commit P99 <500ms");
    println!("  - Visibility delay <1s");
    println!("  - Segment count doesn't explode (<50 segments after 100 commits)");
    println!("\nIf any fail: Tantivy batch-oriented, need LMDB for real-time writes");
    
    Ok(())
}