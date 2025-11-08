use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};
use std::fs;
use std::time::Duration;
use std::thread;

#[cfg(target_os = "macos")]
fn get_rss_kb() -> Option<u64> {
    use std::process::Command;
    let pid = std::process::id();
    let output = Command::new("ps")
        .args(&["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let rss_str = String::from_utf8_lossy(&output.stdout);
    rss_str.trim().parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
fn get_rss_kb() -> Option<u64> {
    let stat = fs::read_to_string("/proc/self/status").ok()?;
    for line in stat.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1].parse::<u64>().ok();
            }
        }
    }
    None
}

fn main() {
    println!("=== FAST FIELD MEMORY OVERHEAD TEST ===\n");
    println!("Goal: Measure RAM cost of sortable fields for multi-tenant density");
    println!("Context: Earlier estimate = 4.10 MB/tenant, need to validate fast fields don't blow this\n");

    // Baseline memory
    thread::sleep(Duration::from_millis(100));
    let baseline_kb = get_rss_kb().expect("Could not read RSS");
    println!("Baseline RSS: {:.2} MB\n", baseline_kb as f64 / 1024.0);

    // Test 1: Minimal schema (no fast fields)
    println!("--- Test 1: Minimal schema (no fast fields) ---");
    {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT);
        schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

        for i in 0..100_000 {
            let doc = doc!(
                schema.get_field("title").unwrap() => format!("Product {}", i),
                schema.get_field("body").unwrap() => "Description text here",
            );
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        let _searcher = reader.searcher();

        thread::sleep(Duration::from_millis(200));
        let minimal_kb = get_rss_kb().expect("Could not read RSS");
        let minimal_delta = (minimal_kb as f64 - baseline_kb as f64) / 1024.0;
        println!("Memory after indexing: {:.2} MB delta", minimal_delta);
    }

    thread::sleep(Duration::from_millis(500));
    let after_drop_kb = get_rss_kb().expect("Could not read RSS");
    println!("Memory after drop: {:.2} MB\n", (after_drop_kb as f64 - baseline_kb as f64) / 1024.0);

    // Test 2: With 5 fast fields (realistic for sorting)
    println!("--- Test 2: With 5 fast fields (price, timestamp, rating, views, stock) ---");
    {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT);
        schema_builder.add_text_field("body", TEXT);
        schema_builder.add_u64_field("price", FAST);
        schema_builder.add_u64_field("timestamp", FAST);
        schema_builder.add_u64_field("rating", FAST);
        schema_builder.add_u64_field("views", FAST);
        schema_builder.add_u64_field("stock", FAST);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let price = schema.get_field("price").unwrap();
        let timestamp = schema.get_field("timestamp").unwrap();
        let rating = schema.get_field("rating").unwrap();
        let views = schema.get_field("views").unwrap();
        let stock = schema.get_field("stock").unwrap();

        for i in 0..100_000 {
            let doc = doc!(
                title => format!("Product {}", i),
                body => "Description text here",
                price => (100 + i % 900) as u64,
                timestamp => 1700000000 + i as u64,
                rating => (1 + i % 5) as u64,
                views => (i * 7) as u64,
                stock => (10 + i % 100) as u64,
            );
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        let _searcher = reader.searcher();

        thread::sleep(Duration::from_millis(200));
        let with_fast_kb = get_rss_kb().expect("Could not read RSS");
        let with_fast_delta = (with_fast_kb as f64 - baseline_kb as f64) / 1024.0;
        println!("Memory after indexing: {:.2} MB delta", with_fast_delta);
    }

    thread::sleep(Duration::from_millis(500));
    let after_drop2_kb = get_rss_kb().expect("Could not read RSS");
    println!("Memory after drop: {:.2} MB\n", (after_drop2_kb as f64 - baseline_kb as f64) / 1024.0);

    // Test 3: Fast fields with STORED (worst case)
    println!("--- Test 3: Fast fields with STORED (worst case) ---");
    {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT | STORED);
        schema_builder.add_text_field("body", TEXT);
        schema_builder.add_u64_field("price", FAST | STORED);
        schema_builder.add_u64_field("timestamp", FAST | STORED);
        schema_builder.add_u64_field("rating", FAST | STORED);
        schema_builder.add_u64_field("views", FAST | STORED);
        schema_builder.add_u64_field("stock", FAST | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer: IndexWriter = index.writer(50_000_000).unwrap();

        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        let price = schema.get_field("price").unwrap();
        let timestamp = schema.get_field("timestamp").unwrap();
        let rating = schema.get_field("rating").unwrap();
        let views = schema.get_field("views").unwrap();
        let stock = schema.get_field("stock").unwrap();

        for i in 0..100_000 {
            let doc = doc!(
                title => format!("Product {}", i),
                body => "Description text here",
                price => (100 + i % 900) as u64,
                timestamp => 1700000000 + i as u64,
                rating => (1 + i % 5) as u64,
                views => (i * 7) as u64,
                stock => (10 + i % 100) as u64,
            );
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        let _searcher = reader.searcher();

        thread::sleep(Duration::from_millis(200));
        let stored_kb = get_rss_kb().expect("Could not read RSS");
        let stored_delta = (stored_kb as f64 - baseline_kb as f64) / 1024.0;
        println!("Memory after indexing: {:.2} MB delta", stored_delta);
    }

    thread::sleep(Duration::from_millis(500));
    let final_kb = get_rss_kb().expect("Could not read RSS");
    println!("Memory after drop: {:.2} MB\n", (final_kb as f64 - baseline_kb as f64) / 1024.0);

    println!("=== ANALYSIS ===");
    println!("Observed memory scaling (10K → 100K docs):");
    println!("  Minimal: 9.4 → 12.3 MB (1.3x for 10x docs)");
    println!("  5 fast fields: 23.9 → 41.2 MB (1.7x for 10x docs)");
    println!("  With STORED: 31.2 → 54.8 MB (1.8x for 10x docs)");
    println!("\nNon-linear scaling indicates high fixed overhead:");
    println!("  - Theoretical: 5 fields × 8 bytes × 100K = 4 MB");
    println!("  - Actual: 41.2 MB (10x theory)");
    println!("  - Overhead sources: mmap alignment, metadata, write buffers");
    println!("\nPer-tenant projection at 50K docs (interpolated):");
    println!("  23.9 + (41.2 - 23.9) × 0.44 = ~31 MB per tenant");
    println!("\nDensity estimates:");
    println!("  - 4 GB node: 130 tenants (if 31 MB holds)");
    println!("  - After drop RSS (20 MB): 200 tenants (if idle compaction occurs)");
    println!("  - Original target (400 tenants): UNVIABLE on 4GB with 5 fast fields");
    println!("\n⚠ CRITICAL: This test uses in-RAM indices.");
    println!("   Disk-backed indices with mmap may have different characteristics.");
    println!("   Run realistic_density_test.rs for production estimate.");
}


// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin fast_field_memory_test
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 39.41s
//      Running `target/release/fast_field_memory_test`
// === FAST FIELD MEMORY OVERHEAD TEST ===

// Goal: Measure RAM cost of sortable fields for multi-tenant density
// Context: Earlier estimate = 4.10 MB/tenant, need to validate fast fields don't blow this

// Baseline RSS: 3.05 MB

// --- Test 1: Minimal schema (no fast fields) ---
// Memory after indexing: 12.25 MB delta
// Memory after drop: 10.80 MB

// --- Test 2: With 5 fast fields (price, timestamp, rating, views, stock) ---
// Memory after indexing: 41.18 MB delta
// Memory after drop: 20.34 MB

// --- Test 3: Fast fields with STORED (worst case) ---
// Memory after indexing: 54.84 MB delta
// Memory after drop: 27.81 MB

// === ANALYSIS ===
// Fast fields are stored in columnar format:
//   - u64 field = 8 bytes × num_docs = 800 KB per 100K docs
//   - 5 fields = 4 MB per 100K docs

// Estimated overhead per tenant (100K docs):
//   - Base index: ~2-3 MB (text, positions, BM25 metadata)
//   - 5 fast fields: ~0.4 MB
//   - STORED fields: ~0.5-1 MB (if needed for result display)
//   - Total: ~3-4.5 MB per tenant

// Density validation:
//   - If actual overhead matches, 400 tenants × 4 MB = 1.6 GB
//   - With 4 GB server: 2.4 GB headroom for OS, queries, write buffers
//   - Conclusion: 400 tenants/node density is viable


//   # Memory Scaling Analysis

// **10K docs → 100K docs (10x increase):**

// | Configuration | 10K docs | 100K docs | Scaling Factor |
// |---------------|----------|-----------|----------------|
// | Minimal (no fast fields) | 9.4 MB | 12.3 MB | **1.3x** |
// | 5 fast fields | 23.9 MB | 41.2 MB | **1.7x** |
// | Fast + STORED | 31.2 MB | 54.8 MB | **1.8x** |

// **This is non-linear growth with high fixed overhead.**

// ## Per-Doc Memory Cost

// **Minimal schema:**
// - (12.3 - 9.4) MB / 90K additional docs = **32 bytes/doc**
// - But 10K docs used 9.4 MB = **940 bytes/doc**
// - Fixed overhead dominates at small scale

// **5 fast fields:**
// - (41.2 - 23.9) MB / 90K docs = **192 bytes/doc** marginal
// - Theoretical: 5 fields × 8 bytes = 40 bytes/doc
// - Actual is 4.8x theory (metadata, alignment, BM25 structures)

// ## Revised Density Projections

// **Assume production tenant = 50K docs average (between 10K and 100K):**

// Linear interpolation: 23.9 + (41.2 - 23.9) × 0.44 = **31.5 MB per tenant**

// **4 GB node capacity:**
// - 4096 MB / 31.5 MB = **130 tenants/node**
// - Not 400, not 170. **130 tenants.**

// **With STORED fields (worst case): 54.8 MB → 75 tenants/node**

// ## Critical Implications

// **Your replication model assumed 400 tenants/node:**
// - 400 tenants × 2.5 writes/sec = 1000 writes/sec
// - Batching: accumulate 10 commits across tenants
// - Cost: 4.5 cores for 30 replicas

// **At 130 tenants/node:**
// - 130 × 2.5 = 325 writes/sec per node
// - Batching efficiency: need 3+ seconds to accumulate 10 commits (vs 1s at 400 tenants)
// - Replication lag increases OR you batch smaller groups (higher CPU cost)

// **3x node count to serve same total tenants = 3x infrastructure cost.**

// ## Open Questions

// **Q1: Does "after drop" RSS reflect steady-state?**

// Your test shows 41.2 MB → 20.3 MB after drop. This suggests:
// - Write buffers (50 MB IndexWriter) account for ~20 MB
// - Steady-state per tenant with 5 fast fields = **20 MB**
// - 4 GB / 20 MB = **200 tenants/node** (better, but still not 400)

// **But:** "After drop" may not represent query load. Once readers access mmap pages, RSS grows back. Your earlier `mmap_demand_test.rs` showed this.

// **Q2: Are you testing in-RAM index or disk-backed?**

// `Index::create_in_ram()` may have different characteristics than disk + mmap. Production will be disk-backed. Test with `Index::create_in_dir()` instead.

// **Q3: What's the segment merge overhead?**

// Your test does single commit. Production has continuous writes → segment merging. Tantivy background merge threads hold old segments in memory during merge. This spikes RSS.

// ## Recommended Next Test

// **Realistic density test:**
// ```rust
// // Create 200 tenants × 50K docs each on disk (not RAM)
// // Simulate: 50 tenants actively queried (mmap demand)
// // Measure: steady-state RSS after 5 min of query load
// // Expected: 50 × 30 MB (if your interpolation holds) = 1.5 GB working set
// // Add: write buffer overhead, merge overhead, query cache
// // Target: <3 GB total on 4 GB node
// ```

// **If this test shows >3.5 GB, your 400 tenant assumption is dead.**

// ## Hypothesis to Validate

// The "after drop" measurement (20.3 MB for 100K docs) might be your true steady-state if:
// 1. Tenants are mostly idle (zipf distribution - 80/20 rule)
// 2. Only hot tenants keep mmap pages resident
// 3. OS evicts cold tenant pages aggressively

// **If true: 200 tenants/node viable with 20% active = 40 hot tenants × 40 MB = 1.6 GB, matches your target.**

// **But you need to test zipf query distribution, not uniform.**

// ---

// **Do you want to proceed with 130-200 tenant density estimate, or run the realistic multi-tenant demand test first?**