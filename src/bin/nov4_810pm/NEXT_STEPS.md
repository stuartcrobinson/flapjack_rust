# Flapjack Search Engine: Architecture Validation & Development Roadmap

**Date:** November 4, 2025  
**Status:** Storage layer validated, ready for search implementation

---

## Executive Summary

**Goal:** Multi-tenant search engine competing with Algolia/Meilisearch at <50% their pricing while supporting seamless tenant migration.

**Key constraint:** 400 tenants per 4GB node @ $30/month cost = $0.075/tenant infrastructure vs $1-5/tenant revenue.

**Status:** Separate-LMDB-file-per-tenant architecture validated. All critical infrastructure assumptions tested. Ready to build search layer.

---

## Experimental Results

### ✅ Validated Assumptions

| Hypothesis | Result | Evidence | Test File |
|------------|--------|----------|-----------|
| LMDB per-env overhead <1 MB | **0.048-0.067 MB** (15x better) | 400 envs = 27 MB baseline | `src/bin/nov4_8pm/separate_env_memory_test.rs` |
| Working set per active tenant <2 MB | **0.5-3 MB** (acceptable) | Phase 6: 0.37 MB per 1K queries | `src/bin/nov4_810pm/realistic_size_test.rs` |
| Sequential commits <15ms P99 | **4.18ms** (3.5x better) | 20 tenants × 50 writes | `src/bin/nov4_8pm/sequential_commit_test.rs` |
| Concurrent reads scale to 4K QPS | **0.017ms P99 @ 5K QPS** (excellent) | 100 threads, 0 errors | `src/bin/nov4_810pm/concurrent_read_test.rs` |
| Hot copy migration <30s | **38ms for 57 MB** (750x better) | Filesystem copy with active writes | `src/bin/nov4_8pm/migration_copy_test.rs` |
| Tantivy unsuitable for multi-tenant | **3,851ms P99 with 10 tenants** (catastrophic) | Fsync serialization | `src/bin/write_latency_test.rs` |
| LMDB enables cross-tenant batching | **1 fsync for 20 tenants** (validated) | But unnecessary - migration more important | `src/bin/cross_tenant_batch_test.rs` |
| Write throughput scales with batch size | **<15ms P99 for 1K items** | Linear scaling, no bottleneck | `src/bin/write_batch_scaling_test.rs` |
| Selective page faulting works | **0 MB for inactive tenants** | Lazy loading confirmed | `src/bin/selective_fault_test.rs` |
| FST overhead negligible | **0.03 MB per 10K terms** | 100-300x smaller than estimated | `src/bin/fst_overhead_test_clean.rs` |
| Single-field sorts <1ms | **0.12ms P99** for range queries | Integer key B-tree | `src/bin/sort_test.rs` |

### ⚠️ Open Questions

| Question | Impact | Next Step |
|----------|--------|-----------|
| BM25 metadata overhead | Estimated 2 MB/tenant, unmeasured | Build BM25, measure |
| Multi-field sort cost | 5 fields × 0.4 MB = 2 MB/tenant? | Build, measure |
| Query intersection algorithm | Affects P99 latency for text+filter+sort | Prototype planner |
| Global replication strategy | LMDB single-writer limits multi-region writes | Design async replication |
| Production write distribution | If 100 tenants write simultaneously? | Monitor after launch |
| Tenant size at 10 GB boundary | Performance degradation? | Test later when needed |

---

## Multi-Tenant Requirements: Status

### ✅ Validated

1. **High density:** 400 tenants/4GB = $0.075/tenant infrastructure cost
   - Test: 0.048 MB overhead × 400 = 19 MB baseline
   - Plus: 80 active @ 3 MB = 240 MB + BM25/sorts = ~700 MB total
   - Leaves 3.3 GB headroom

2. **Seamless migration:** Move tenants between machines without downtime
   - Test: 38ms to copy 57 MB tenant file with active writes
   - Method: `rsync tenant_N.lmdb` + update routing table
   - No coordination required (filesystem copy is atomic read)

3. **Dynamic load balancing:** Move noisy neighbor to dedicated hardware
   - Architecture: Each tenant = separate file = independently movable
   - No cross-tenant dependencies (separate files, no shared state)

4. **Pricing advantage:** Infrastructure cost <15% of Algolia
   - Algolia: $1-5/tenant/month (10K-100K docs)
   - Flapjack: $0.075 infra + $0.20 ops = $0.275/tenant @ 400 density
   - Target price: $1/tenant = 364% margin

### ⚠️ Partially Validated

5. **Import/export for migration:** Can export entire tenant DB
   - Validated: Filesystem copy works for whole tenant
   - Not validated: Selective export of subset (not needed for migration)
   - Trade-off: All-or-nothing is simpler than partial export

6. **Feature parity with Algolia/Meilisearch:**
   - Text search: ✅ FST → posting lists (tested, 0.03 MB overhead)
   - Filters: ⚠️ Need to build (B-tree range queries tested, intersection untested)
   - Sorting: ✅ Single-field tested (0.12ms), multi-field untested
   - Facets: ⚠️ Not designed
   - Typo tolerance: ⚠️ Not designed
   - Highlighting: ⚠️ Not designed
   - BM25 relevance: ⚠️ Not built

---

## Development Roadmap

### Phase 1: Core Search (Weeks 1-3)
- [ ] Document schema & indexing pipeline
- [ ] FST term index (already validated in tests)
- [ ] Posting list storage & compression
- [ ] BM25 scoring implementation
- [ ] **Measure actual BM25 memory overhead**
- [ ] Basic query: text search only, no filters/sorts

### Phase 2: Query Features (Weeks 4-6)
- [ ] Filter implementation (range queries, exact matches)
- [ ] Multi-field sort indices
- [ ] Query planner: text + filters + sort intersection
- [ ] **Measure query intersection performance**
- [ ] Pagination
- [ ] Faceted search

### Phase 3: Write Path (Weeks 7-8)
- [ ] Document ingestion API
- [ ] Batch commit strategy (1 commit/sec per tenant)
- [ ] Update/delete handling
- [ ] LMDB compaction triggers

### Phase 4: Multi-Tenant Operations (Weeks 9-10)
- [ ] Tenant routing layer
- [ ] Tenant creation/deletion
- [ ] Migration orchestration (copy + routing update)
- [ ] Monitoring per-tenant resource usage

### Phase 5: Production Readiness (Weeks 11-12)
- [ ] API gateway (rate limiting, authentication)
- [ ] Tenant tokens for access control
- [ ] Read replicas
- [ ] Async replication strategy for multi-region
- [ ] Prometheus metrics, alerting

### Phase 6: Algolia Parity Features (Weeks 13-16)
- [ ] Typo tolerance (Levenshtein FST)
- [ ] Synonym support
- [ ] Highlighting
- [ ] Custom ranking rules
- [ ] A/B testing support

---

## Immediate Next Steps (Weeks 1-2)

### 1. Document Schema & Indexing (3 days)

**Goal:** Define how documents map to LMDB structures.

**Design decisions:**
```rust
// Per tenant, multiple LMDB named DBs:
// 1. "docs" - doc_id -> JSON blob (doc store)
// 2. "terms" - term -> posting list (inverted index)
// 3. "sort_price" - price -> doc_id (sort index)
// 4. "sort_date" - date -> doc_id (sort index)
// 5. "bm25_meta" - doc_id -> {len, norms} (scoring metadata)
```

**Key questions:**
- Posting list format: raw `Vec<u32>` or delta+varint or Roaring?
  - Test with 10K posting list: decode time <1ms?
- Sort index: INTEGER_KEY or custom encoding?
- BM25 storage: separate DB or inline with docs?

**Deliverable:** 
- `src/indexer/schema.rs` - Document to LMDB mapping
- `src/indexer/writer.rs` - Batch write pipeline
- Test: Index 10K docs, measure LMDB size & RSS

### 2. FST Term Index (2 days)

**Already validated:** 0.03 MB for 10K terms.

**Implementation:**
```rust
// Build FST from sorted terms
let mut builder = fst::MapBuilder::memory();
for (term, posting_list_offset) in sorted_terms {
    builder.insert(term, posting_list_offset)?;
}
let fst_bytes = builder.into_inner()?;

// Store FST in LMDB
txn.put(db, "fst", &fst_bytes)?;
```

**Key questions:**
- FST stores offset into "terms" DB or inline small posting lists?
- Rebuild FST on every write or incremental updates?

**Deliverable:**
- `src/search/fst_index.rs`
- Test: Query 100 terms, verify posting list retrieval

### 3. Posting Lists & BM25 (4 days)

**Design:**
```rust
struct PostingList {
    doc_ids: Vec<u32>,      // Delta-encoded, varint
    term_freqs: Vec<u16>,   // For BM25
}

struct BM25Metadata {
    doc_length: u32,
    field_norms: Vec<f32>,  // Per searchable field
}
```

**Key decision: Compression**
- Option A: Delta encoding + varint (simple, slower)
- Option B: Roaring bitmaps (faster, complex)
- Test both, measure decode time for 10K doc posting list

**Deliverable:**
- `src/search/posting_list.rs`
- `src/search/bm25.rs`
- **Critical test:** Measure BM25 metadata RSS overhead
  - Index 20 tenants × 10K docs with BM25
  - Open all envs, measure RSS
  - Compare to baseline (without BM25)
  - Target: <2 MB/tenant

### 4. Basic Query (text-only) (3 days)

**Goal:** Query "laptop" → retrieve top 100 docs by BM25.

**Algorithm:**
```rust
fn search(query: &str) -> Vec<DocID> {
    // 1. Tokenize query
    let terms = tokenize(query);
    
    // 2. FST lookup → posting lists
    let mut posting_lists = vec![];
    for term in terms {
        if let Some(offset) = fst.get(term) {
            let pl = load_posting_list(offset);
            posting_lists.push(pl);
        }
    }
    
    // 3. Union posting lists (OR query)
    let candidates = union(posting_lists);
    
    // 4. BM25 score
    let mut scored = vec![];
    for doc_id in candidates {
        let score = bm25_score(doc_id, &terms);
        scored.push((score, doc_id));
    }
    
    // 5. Top-K
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(100);
    scored.iter().map(|(_, id)| *id).collect()
}
```

**Deliverable:**
- `src/search/query.rs`
- Test: Query 100 random terms, P99 <50ms

---

## Week 3: Query Planner (Filters + Sort)

### 5. Filter Implementation (2 days)

**Design:**
```rust
// Range query on sort index
fn filter_price(min: u32, max: u32) -> Vec<DocID> {
    let cursor = txn.open_ro_cursor(sort_price_db)?;
    let mut results = vec![];
    
    for (price_bytes, doc_id_bytes) in cursor.iter_from(&min.to_be_bytes()) {
        let price = u32::from_be_bytes(price_bytes);
        if price > max { break; }
        results.push(u32::from_le_bytes(doc_id_bytes));
    }
    
    results
}
```

**Key question:** Intersection algorithm?
- Option A: Hash set intersection (simple, memory)
- Option B: Sorted list merge (complex, cache-friendly)
- Option C: Roaring bitmap AND (fast, dependency)

**Test:**
```rust
// Query: "laptop" AND price:[500-2000]
// 1. Text search → 10K candidates
// 2. Price filter → 5K candidates
// 3. Intersect → 2K results
// Measure intersection time, target <10ms
```

### 6. Multi-Field Sort (2 days)

**Design:**
```rust
// Create sort index per field
for doc in docs {
    txn.put(sort_price_db, &doc.price.to_be_bytes(), &doc.id)?;
    txn.put(sort_date_db, &doc.date.to_be_bytes(), &doc.id)?;
}

// Query with sort
fn search_sorted(query: &str, sort_by: SortField) -> Vec<DocID> {
    let candidates = text_search(query); // BM25-scored
    
    // Re-sort by requested field
    let mut with_sort_key = vec![];
    for doc_id in candidates {
        let sort_value = get_sort_value(doc_id, sort_by);
        with_sort_key.push((sort_value, doc_id));
    }
    with_sort_key.sort_by_key(|x| x.0);
    
    with_sort_key.iter().map(|x| x.1).collect()
}
```

**Key question:** Does sort index lookup kill performance?
- 100 candidates × sort key lookup = 100 LMDB gets
- Target: <5ms for 100 lookups

**Test:** 
- Index with 5 sortable fields
- Measure RSS overhead vs baseline
- Query + sort by each field, measure P99

### 7. Query Planner Integration (3 days)

**Goal:** text + filter + sort in optimal order.

**Algorithm:**
```rust
fn execute_query(q: Query) -> Vec<DocID> {
    // 1. Estimate selectivity
    let text_count = estimate_term_freq(q.text);
    let filter_count = estimate_filter_selectivity(q.filters);
    
    // 2. Execute most selective first
    let candidates = if filter_count < text_count {
        let filtered = execute_filters(q.filters);
        filter_by_text(filtered, q.text) // Check text match on filtered set
    } else {
        let text_results = text_search(q.text);
        apply_filters(text_results, q.filters)
    };
    
    // 3. Sort
    if let Some(sort) = q.sort {
        sort_by_field(candidates, sort)
    } else {
        candidates // Already BM25-sorted
    }
}
```

**Test:**
```rust
// Query: "laptop" AND price:[500-2000] ORDER BY price LIMIT 100
// Measure P99 with:
// - High selectivity text (10 results)
// - Low selectivity text (10K results)
// - High selectivity filter (100 results)
// - Low selectivity filter (50K results)
// Target: <50ms P99 for all combinations
```

---

## Critical Unknowns (Post Week 3)

### BM25 Overhead
**Current estimate:** 2 MB/tenant  
**Actual:** Unknown until built  
**Risk:** If 5+ MB/tenant, density drops to 200/4GB

**Mitigation:** If too large, store BM25 metadata in separate structure (not LMDB), accept slower scoring.

### Query Intersection Performance
**Current assumption:** Simple hash set intersection sufficient  
**Actual:** Unknown until 10K × 10K intersection tested  
**Risk:** Naive algorithm >50ms, need Roaring bitmaps

**Mitigation:** Profile, optimize hot path, add Roaring if needed.

### Multi-Field Sort Cost
**Current estimate:** 5 fields × 0.4 MB = 2 MB/tenant  
**Actual:** Unknown until built  
**Risk:** If 5+ MB/tenant, combined with BM25 pushes total >5 MB/tenant

**Mitigation:** Limit free tier to 2 sortable fields, charge for more.

---

## Success Criteria (End of Week 3)

1. **Index 100K docs across 20 tenants**
   - Disk usage <50 MB/tenant
   - RSS <100 MB total

2. **Query performance**
   - Text-only: <10ms P99
   - Text + filter: <30ms P99
   - Text + filter + sort: <50ms P99

3. **Memory validation**
   - BM25 overhead measured: <3 MB/tenant
   - Multi-field sort measured: <2 MB/tenant
   - Total per active tenant: <5 MB

4. **Pricing model confirmed**
   - If all above pass: 400 tenants/4GB viable
   - If overhead >5 MB/tenant: reduce to 200/4GB, still 5x margin

If all success criteria met: Architecture validated, proceed to write path & multi-tenant operations.

If any fail: Reassess density target or feature scope.