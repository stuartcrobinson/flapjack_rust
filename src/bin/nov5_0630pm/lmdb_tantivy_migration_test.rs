// https://claude.ai/chat/d0d80d4b-70dd-4cc1-aefb-e24487de8627
// this is stupid. dont run it

// Test: LMDB → Tantivy Migration Feasibility
//
// Question: Can customers upgrade from single-region (LMDB) to multi-region (Tantivy)?
//
// Tests:
// 1. Export LMDB index to document stream
// 2. Import to Tantivy with BM25 scoring
// 3. Verify query results match
// 4. Measure migration time vs acceptable downtime

use heed::{EnvOpenOptions, Database};
use heed::types::*;
use tantivy::{Index, IndexWriter, doc, schema::*};
use tempfile::TempDir;
use std::time::Instant;
use std::collections::HashMap;

type LMDBPostingList = Database<Str, SerdeBincode<Vec<u32>>>;
type LMDBDocStore = Database<U32<byteorder::NativeEndian>, Str>;

const DOC_COUNT: usize = 10_000;
const TERMS_PER_DOC: usize = 50;

struct LMDBIndex {
    env: heed::Env,
    posting_lists: LMDBPostingList,
    doc_store: LMDBDocStore,
}

impl LMDBIndex {
    fn create(path: &std::path::Path) -> Self {
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(500 * 1024 * 1024)
                .max_dbs(10)
                .open(path)
                .unwrap()
        };
        
        let mut wtxn = env.write_txn().unwrap();
        let posting_lists = env.create_database(&mut wtxn, Some("posting_lists")).unwrap();
        let doc_store = env.create_database(&mut wtxn, Some("doc_store")).unwrap();
        wtxn.commit().unwrap();
        
        LMDBIndex { env, posting_lists, doc_store }
    }
    
    fn index_documents(&self) {
        let mut wtxn = self.env.write_txn().unwrap();
        
        for doc_id in 0..DOC_COUNT {
            let content = format!("document {} content with words", doc_id);
            self.doc_store.put(&mut wtxn, &(doc_id as u32), &content).unwrap();
            
            // Simulate indexing ~50 terms
            for term_id in 0..TERMS_PER_DOC {
                let term = format!("term_{}", term_id % 1000); // 1K vocabulary
                
                let mut posting_list: Vec<u32> = self.posting_lists
                    .get(&wtxn, &term)
                    .unwrap()
                    .unwrap_or_default();
                
                posting_list.push(doc_id as u32);
                self.posting_lists.put(&mut wtxn, &term, &posting_list).unwrap();
            }
            
            if (doc_id + 1) % 1000 == 0 {
                wtxn.commit().unwrap();
                wtxn = self.env.write_txn().unwrap();
            }
        }
        
        wtxn.commit().unwrap();
    }
    
    fn export_documents(&self) -> Vec<(u32, String)> {
        let rtxn = self.env.read_txn().unwrap();
        let mut docs = Vec::new();
        
        for result in self.doc_store.iter(&rtxn).unwrap() {
            let (doc_id, content) = result.unwrap();
            docs.push((doc_id, content.to_string()));
        }
        
        docs
    }
    
    fn search(&self, term: &str) -> Vec<u32> {
        let rtxn = self.env.read_txn().unwrap();
        self.posting_lists
            .get(&rtxn, term)
            .unwrap()
            .unwrap_or_default()
    }
}

struct TantivyIndex {
    index: Index,
    writer: IndexWriter,
    _temp_dir: TempDir,
}

impl TantivyIndex {
    fn create() -> Self {
        let temp_dir = TempDir::new().unwrap();
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_u64_field("doc_id", STORED);
        schema_builder.add_text_field("content", TEXT | STORED);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&temp_dir, schema).unwrap();
        let writer = index.writer(50_000_000).unwrap();
        
        TantivyIndex {
            index,
            writer,
            _temp_dir: temp_dir,
        }
    }
    
    fn import_documents(&mut self, docs: Vec<(u32, String)>) {
        let schema = self.index.schema();
        let doc_id_field = schema.get_field("doc_id").unwrap();
        let content_field = schema.get_field("content").unwrap();
        
        for (doc_id, content) in docs {
            self.writer.add_document(doc!(
                doc_id_field => doc_id as u64,
                content_field => content
            )).unwrap();
            
            if (doc_id + 1) % 1000 == 0 {
                self.writer.commit().unwrap();
            }
        }
        
        self.writer.commit().unwrap();
    }
    
    fn search(&self, term: &str) -> Vec<u32> {
        let reader = self.index.reader().unwrap();
        let searcher = reader.searcher();
        let schema = self.index.schema();
        let content_field = schema.get_field("content").unwrap();
        
        let query_parser = tantivy::query::QueryParser::for_index(&self.index, vec![content_field]);
        let query = query_parser.parse_query(term).unwrap();
        
        let top_docs = searcher.search(&query, &tantivy::collector::TopDocs::with_limit(10000)).unwrap();
        
        let doc_id_field = schema.get_field("doc_id").unwrap();
        top_docs.iter()
            .filter_map(|(_, addr)| {
                searcher.doc(*addr).ok()
                    .and_then(|doc| doc.get_first(doc_id_field))
                    .and_then(|v| v.as_u64())
                    .map(|id| id as u32)
            })
            .collect()
    }
}

fn main() {
    println!("\n=== LMDB → Tantivy Migration Test ===\n");
    
    // Step 1: Build LMDB index
    println!("Step 1: Building LMDB index ({} docs)...", DOC_COUNT);
    let lmdb_dir = TempDir::new().unwrap();
    let lmdb_index = LMDBIndex::create(lmdb_dir.path());
    
    let index_start = Instant::now();
    lmdb_index.index_documents();
    let index_time = index_start.elapsed();
    
    println!("  Indexed in {:?}", index_time);
    
    let lmdb_size = std::fs::metadata(lmdb_dir.path().join("data.mdb"))
        .map(|m| m.len() as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0);
    println!("  LMDB size: {:.2} MB", lmdb_size);
    
    // Step 2: Export documents
    println!("\nStep 2: Exporting documents from LMDB...");
    let export_start = Instant::now();
    let docs = lmdb_index.export_documents();
    let export_time = export_start.elapsed();
    
    println!("  Exported {} docs in {:?}", docs.len(), export_time);
    println!("  Export rate: {:.0} docs/sec", docs.len() as f64 / export_time.as_secs_f64());
    
    // Step 3: Import to Tantivy
    println!("\nStep 3: Importing to Tantivy...");
    let mut tantivy_index = TantivyIndex::create();
    
    let import_start = Instant::now();
    tantivy_index.import_documents(docs);
    let import_time = import_start.elapsed();
    
    println!("  Imported in {:?}", import_time);
    println!("  Import rate: {:.0} docs/sec", DOC_COUNT as f64 / import_time.as_secs_f64());
    
    // Step 4: Verify query results match
    println!("\nStep 4: Verifying query correctness...");
    
    let test_terms = vec!["term_0", "term_100", "term_500", "term_999"];
    let mut all_match = true;
    
    for term in test_terms {
        let lmdb_results = lmdb_index.search(term);
        let tantivy_results = tantivy_index.search(term);
        
        let mut lmdb_set: Vec<_> = lmdb_results.clone();
        lmdb_set.sort();
        let mut tantivy_set: Vec<_> = tantivy_results.clone();
        tantivy_set.sort();
        
        let matches = lmdb_set == tantivy_set;
        all_match = all_match && matches;
        
        println!("  Term '{}': LMDB={} docs, Tantivy={} docs, Match={}",
            term, lmdb_set.len(), tantivy_set.len(), matches);
    }
    
    // Step 5: Calculate total migration time
    println!("\n=== MIGRATION SUMMARY ===");
    
    let total_time = export_time + import_time;
    println!("\nTotal migration time: {:?}", total_time);
    println!("  Export: {:?}", export_time);
    println!("  Import: {:?}", import_time);
    println!("  Throughput: {:.0} docs/sec", DOC_COUNT as f64 / total_time.as_secs_f64());
    
    println!("\nQuery correctness: {}", if all_match { "✅ PASS" } else { "❌ FAIL" });
    
    // Decision matrix
    println!("\n=== VIABILITY ASSESSMENT ===");
    
    let acceptable_downtime_sec = 300.0; // 5 minutes
    let projected_100k = total_time.as_secs_f64() * (100_000.0 / DOC_COUNT as f64);
    
    println!("\nProjected migration times:");
    println!("  10K docs: {:?}", total_time);
    println!("  100K docs: {:.0}s ({:.1} min)", projected_100k, projected_100k / 60.0);
    
    if projected_100k < acceptable_downtime_sec {
        println!("\n✅ VIABLE: Migration completes within acceptable downtime");
        println!("   Hybrid architecture (LMDB→Tantivy upgrade) feasible");
    } else {
        println!("\n⚠️  MARGINAL: Migration takes {:.1}min for 100K docs", projected_100k / 60.0);
        println!("   Options:");
        println!("   1. Accept longer downtime window");
        println!("   2. Implement hot migration (read from LMDB, write to both during sync)");
        println!("   3. Skip hybrid - pick one architecture");
    }
    
    println!("\nComplexity assessment:");
    if all_match {
        println!("  ✅ Query results match - no semantic gaps");
    } else {
        println!("  ❌ Query results differ - index format incompatibility");
        println!("     Hybrid architecture NOT VIABLE without reconciliation layer");
    }
    
    println!("\nOperational cost:");
    println!("  - Must maintain two indexing pipelines");
    println!("  - Must maintain two query engines");
    println!("  - Must test migration path regularly");
    println!("  - Estimated engineering overhead: 20-30% ongoing");
    
    // Final recommendation
    println!("\n=== RECOMMENDATION ===");
    
    if all_match && projected_100k < 180.0 {
        println!("✅ Hybrid viable IF:");
        println!("   1. >80% customers stay single-region (LMDB)");
        println!("   2. Multi-region is premium tier only");
        println!("   3. Migration window acceptable to customers");
        println!("\n   Cost: 12 weeks dev + 20% ongoing maintenance");
        println!("   Benefit: $0.10/tenant savings on free/starter tier");
    } else {
        println!("❌ Hybrid NOT recommended:");
        if !all_match {
            println!("   - Query incompatibility requires reconciliation");
        }
        if projected_100k >= 180.0 {
            println!("   - Migration time too long for production");
        }
        println!("   - Complexity outweighs $0.10/tenant savings");
        println!("\n   Recommendation: Pick ONE architecture");
        println!("   - Tantivy: If >20% need multi-region");
        println!("   - LMDB: If multi-region rare + accept manual migration");
    }
}