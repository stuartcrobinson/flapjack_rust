// src/bin/bm25_memory_test.rs
// Measure actual BM25 metadata overhead per tenant
// Goal: Validate <2-3 MB/tenant assumption for 10K docs

use anyhow::Result;
use rand::Rng;
use std::fs;
use std::path::PathBuf;

use flapjack_rust::bm25::BM25Index;
// use flapjack_rust::bm25::*;
// use bm25::BM25Index;

// Import from your existing test utilities
fn get_rss_mb() -> f64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            return kb as f64 / 1024.0;
        }
    }
    0.0
}

fn generate_document(doc_id: u32, avg_words: usize, vocab_size: usize) -> (u32, Vec<String>) {
    let mut rng = rand::thread_rng();
    let num_words = (avg_words as i32 + rng.gen_range(-10..10)).max(10) as usize;
    
    let tokens: Vec<String> = (0..num_words)
        .map(|_| format!("word{}", rng.gen_range(0..vocab_size)))
        .collect();
    
    (doc_id, tokens)
}

fn main() -> Result<()> {
    println!("=== BM25 Memory Overhead Test ===\n");
    println!("Goal: Measure actual memory per tenant for 10K documents");
    println!("Target: <3 MB/tenant for 400 tenant density\n");

    // Configuration
    const TENANTS: usize = 20;
    const DOCS_PER_TENANT: usize = 10_000;
    const AVG_DOC_LENGTH: usize = 100; // tokens
    const VOCAB_SIZE: usize = 5_000; // unique terms per tenant
    const MAP_SIZE: usize = 100 * 1024 * 1024; // 100 MB per tenant

    let base_dir = PathBuf::from("/tmp/flapjack_bm25_test");
    fs::create_dir_all(&base_dir)?;

    // Measure baseline RSS
    println!("Phase 1: Baseline RSS measurement");
    let baseline_rss = get_rss_mb();
    println!("  Baseline RSS: {:.2} MB\n", baseline_rss);

    // Phase 2: Create indexes without opening them
    println!("Phase 2: Creating {} tenant indexes...", TENANTS);
    let mut tenant_paths = Vec::new();
    
    for tenant_id in 0..TENANTS {
        let tenant_path = base_dir.join(format!("tenant_{}", tenant_id));
        tenant_paths.push(tenant_path.clone());
        
        let index = BM25Index::open(&tenant_path, MAP_SIZE)?;
        
        // Generate and index documents in batches
        let batch_size = 1000;
        for batch_start in (0..DOCS_PER_TENANT).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(DOCS_PER_TENANT);
            let docs: Vec<_> = (batch_start..batch_end)
                .map(|i| generate_document(i as u32, AVG_DOC_LENGTH, VOCAB_SIZE))
                .collect();
            
            index.index_documents(docs)?;
        }
        
        if tenant_id % 5 == 0 {
            let stats = index.get_stats()?;
            println!("  Tenant {}: {} docs, {} terms", 
                tenant_id, stats.total_docs, index.count_terms()?);
        }
    }
    
    println!("  All indexes created\n");

    // Phase 3: Open all tenant indexes and measure RSS
    println!("Phase 3: Opening all {} tenant indexes...", TENANTS);
    let rss_before_open = get_rss_mb();
    println!("  RSS before opening: {:.2} MB", rss_before_open);
    
    let mut indexes = Vec::new();
    for tenant_path in &tenant_paths {
        let index = BM25Index::open(tenant_path, MAP_SIZE)?;
        indexes.push(index);
    }
    
    let rss_after_open = get_rss_mb();
    println!("  RSS after opening: {:.2} MB", rss_after_open);
    println!("  Delta (open overhead): {:.2} MB", rss_after_open - rss_before_open);
    println!("  Per tenant: {:.3} MB\n", (rss_after_open - rss_before_open) / TENANTS as f64);

    // Phase 4: Perform queries on all tenants (triggers page faults)
    println!("Phase 4: Running queries on all tenants...");
    let query_terms = vec!["word100".to_string(), "word200".to_string()];
    
    for (i, index) in indexes.iter().enumerate() {
        let results = index.search(&query_terms, 10)?;
        if i == 0 {
            println!("  Sample results: {} docs matched", results.len());
        }
    }
    
    let rss_after_queries = get_rss_mb();
    println!("  RSS after queries: {:.2} MB", rss_after_queries);
    println!("  Delta (active tenant overhead): {:.2} MB", rss_after_queries - rss_after_open);
    println!("  Per active tenant: {:.3} MB\n", (rss_after_queries - rss_after_open) / TENANTS as f64);

    // Phase 5: Heavy query workload
    println!("Phase 5: Heavy query workload (1000 queries per tenant)...");
    let mut total_results = 0;
    
    for index in &indexes {
        for i in 0..1000 {
            let term = format!("word{}", i % VOCAB_SIZE);
            let results = index.search(&[term], 100)?;
            total_results += results.len();
        }
    }
    
    let rss_final = get_rss_mb();
    println!("  Total results: {}", total_results);
    println!("  RSS final: {:.2} MB", rss_final);
    println!("  Working set per tenant: {:.3} MB\n", (rss_final - rss_after_open) / TENANTS as f64);

    // Phase 6: Analyze disk usage
    println!("Phase 6: Disk usage analysis");
    let mut total_disk_mb = 0.0;
    
    for (i, tenant_path) in tenant_paths.iter().enumerate() {
        let size = fs::metadata(tenant_path.join("data.mdb"))?.len();
        let size_mb = size as f64 / (1024.0 * 1024.0);
        total_disk_mb += size_mb;
        
        if i < 3 {
            println!("  Tenant {}: {:.2} MB", i, size_mb);
        }
    }
    
    println!("  Total disk: {:.2} MB", total_disk_mb);
    println!("  Avg per tenant: {:.2} MB\n", total_disk_mb / TENANTS as f64);

    // Phase 7: Memory breakdown analysis
    println!("=== SUMMARY ===");
    println!("Configuration:");
    println!("  {} tenants × {} docs = {} total docs", TENANTS, DOCS_PER_TENANT, TENANTS * DOCS_PER_TENANT);
    println!("  Avg doc length: {} tokens", AVG_DOC_LENGTH);
    println!("  Vocab size: {} unique terms\n", VOCAB_SIZE);
    
    println!("Memory overhead per tenant:");
    let open_overhead = (rss_after_open - rss_before_open) / TENANTS as f64;
    let working_set = (rss_final - rss_after_open) / TENANTS as f64;
    let total_memory = open_overhead + working_set;
    
    println!("  Open (passive):  {:.3} MB", open_overhead);
    println!("  Working set:     {:.3} MB", working_set);
    println!("  Total per tenant: {:.3} MB", total_memory);
    println!("  Disk per tenant: {:.2} MB\n", total_disk_mb / TENANTS as f64);
    
    // Projected capacity
    let node_ram_gb = 4.0;
    let system_overhead_mb = 500.0;
    let available_mb = node_ram_gb * 1024.0 - system_overhead_mb;
    let max_tenants = (available_mb / total_memory).floor() as usize;
    
    println!("Capacity projection (4GB node):");
    println!("  Available RAM: {:.0} MB", available_mb);
    println!("  Max tenants: {}", max_tenants);
    println!("  Infrastructure cost: ${:.3}/tenant @ $30/month", 30.0 / max_tenants as f64);
    
    // Verdict
    println!("\n=== VERDICT ===");
    if total_memory <= 3.0 {
        println!("✅ PASS: {:.3} MB/tenant meets <3 MB target", total_memory);
        println!("   400 tenant density: VIABLE");
    } else if total_memory <= 5.0 {
        println!("⚠️  MARGINAL: {:.3} MB/tenant exceeds 3 MB but acceptable", total_memory);
        println!("   {} tenant density achievable", max_tenants);
        println!("   Still profitable at $1/tenant pricing");
    } else {
        println!("❌ FAIL: {:.3} MB/tenant too high", total_memory);
        println!("   Need to optimize or reduce density target");
    }

    // Cleanup
    fs::remove_dir_all(&base_dir)?;
    
    Ok(())
}
