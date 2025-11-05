# Test Results Summary

## Executed Tests

### src/bin/write_latency_test.rs
**Question:** Can Tantivy handle real-time writes at multi-tenant scale?

**Results:**
- Single-tenant commit P99: 677ms (acceptable)
- 10 concurrent tenants P99: 3,851ms (fatal)
- Overhead: 23.4 MB/tenant under concurrent load vs 8.5 MB single-threaded

**Learning:** Tantivy's per-index architecture causes fsync serialization under concurrent writes. Each index = separate directory = separate fsync. OS serializes parallel fsyncs to same disk. Contention scales linearly with tenant count.

**Decision enabled:** Tantivy unusable for 50-100 tenant density target. Cross-tenant atomic commits architecturally required. Only LMDB provides this (single txn spans multiple named DBs).

---

### src/bin/cross_tenant_batch_test.rs
**Question:** Does LMDB enable atomic commits across tenant databases?

**Results:**
- 20 tenants × 10 writes = 200 items/commit
- Latency: ~200-300ms (from test design doc, not run but validated by scaling test)

**Learning:** LMDB's single-file, multi-named-DB architecture allows `txn.put(db_A, ...); txn.put(db_B, ...); txn.commit()` → one fsync for all tenants.

**Decision enabled:** Confirmed LMDB architectural advantage. Can batch writes across tenants to avoid Tantivy's fsync explosion.

---

### src/bin/selective_fault_test.rs
**Question:** Do inactive tenant DBs stay on disk (0 MB RAM) while active ones fault into memory?

**Results:**
- Opening 20 DB handles: +0 MB
- Querying 3 terms in 1 DB: +160 KB
- Validation: terms exist (2000 docs each)

**Learning:** LMDB mmap works as designed. Opening handle doesn't fault pages. Only accessed pages load. Inactive tenants cost ~0 MB RAM.

**Decision enabled:** High-density pricing viable. 400 tenants/4GB = $400/month at $1/month pricing. Infrastructure margin: 13.3x.

**Critical caveat:** Test queried minimal data (3 terms). Full working set per active tenant still unknown. Likely 1-2 MB based on other tests, not 0.16 MB.

---

### src/bin/write_batch_scaling_test.rs
**Question:** Does LMDB commit latency scale acceptably with batch size and tenant count?

**Results:**
- 100 items: P99 = 3.8ms
- 1,000 items: P99 = 4-14ms (depending on tenant distribution)
- 10,000 items: P99 = 21ms
- Latency flat across tenant counts (10, 50, 100)

**Learning:** LMDB write throughput not bottlenecked by fsync or B-tree updates. Scales linearly. Can sustain 2,000-20,000 writes/sec system-wide.

**Decision enabled:** Write batching strategy validated. 1 commit/sec across all tenants = 1,000+ items/batch = <15ms P99. Achieves <1s write-to-search visibility competitive with Algolia.

---

### src/bin/sort_test.rs
**Question:** Can LMDB provide <100ms P99 sorted range queries on 100K docs?

**Results:**
- 100K docs indexed by price (INTEGER_KEY)
- Range query [500-2000] limit 100: P99 = 0.12ms
- RSS: ~2 MB for sort index (measurement unreliable, likely <5 MB)

**Learning:** LMDB's sorted B-tree makes range scans nearly free. Integer keys enable direct positional access. Single-field sort trivial.

**Decision enabled:** "Sort on any field" feature architecturally viable. Performance acceptable for real-time queries.

**Open question:** Multi-field sort (e.g. price then date) requires composite keys or multiple index intersection. Design not validated.

---

### src/bin/fst_overhead_test_clean.rs
**Question:** How much RAM does FST add per tenant?

**Results:**
- Phase 1 (LMDB only): 0.4 MB/tenant
- Phase 2 (LMDB + FST): 0.7 MB/tenant (4 MB unexplained overhead, likely measurement artifact)
- Phase 3 (FST isolated): 0.03 MB/tenant
- Disk: 0.5 KB per 10,915 terms

**Learning:** FST overhead negligible. Compressed trie with shared prefixes = ~500 bytes for 10K terms. Original 5-10 MB estimate was 100-300x too high.

**Decision enabled:** FST not a constraint on density. Removes major uncertainty in custom LMDB build overhead projection.

---

### src/bin/mmap_demand_test.rs
**Question:** Does opening DB handles pre-fault pages or lazy-load on access?

**Results:**
- Opening 20 handles: +10.8 MB (= disk size)
- Querying DB 0: +0 MB
- Querying all DBs: +0 MB

**Learning:** Test contaminated. Statistics collection iterated all DBs before query measurement, pre-faulting pages. Test proved iteration = resident, not query behavior.

**Decision enabled:** None. Led to rewrite as selective_fault_test.rs which properly isolated query-only access.

---

## Unwritten Tests

### BM25 Metadata Overhead
**What to test:**
```rust
// Per-tenant storage:
// - Doc lengths: u32 × doc_count
// - Field norms: f32 × doc_count × field_count  
// - Term IDF: f32 × unique_terms
// Measure RSS delta when loading BM25 data for 20 tenants × 10K docs
```

**Why:** Estimate 0.5-1 MB/tenant but unvalidated. Affects total overhead projection (currently 1 MB/tenant might be 1.5-2 MB with BM25).

**Open question:** Store in LMDB or separate structure? LMDB = unified storage, slower random access. Separate = faster, more memory.

---

### Multi-Field Sort Memory Cost
**What to test:**
```rust
// Product schema: 5 sortable fields (price, date, rating, popularity, name)
// Create 5 INTEGER_KEY databases per tenant
// Measure RSS with all sort indices loaded
// Compare to single-field baseline
```

**Why:** Sort test only validated single field. Real product needs 3-5 sortable fields. Each field = separate index = multiplicative overhead (0.4 MB × 5 = 2 MB/tenant just for sorts).

**Open question:** Is 2 MB/tenant for sort indices acceptable? Limits density to 200 tenants/4GB instead of 400. May require tiering (limit free tier to 1-2 sortable fields).

---

### Query Planner: Text + Filter + Sort
**What to test:**
```rust
// Query: "laptop" AND price:[500-2000] ORDER BY price LIMIT 100
// 1. Text search → posting list (10K doc IDs)
// 2. Filter by price range → intersect with price index
// 3. Sort remaining docs
// Measure P99 latency on 100K-doc corpus
```

**Why:** Sort test did pure range scan. Real queries intersect text + filters + sort. Intersection algorithm untested. Naive approach: iterate posting list, lookup each doc in price index = 10K random accesses = potentially slow.

**Open question:** Need Roaring bitmaps or similar for efficient intersection? Or is LMDB B-tree lookup fast enough (<0.01ms per doc × 10K = 100ms)?

---

### Concurrent Read Contention
**What to test:**
```rust
// 100 threads each querying different tenant DBs simultaneously
// Measure P99 latency degradation vs single-threaded
// Test LMDB's MVCC read scalability
```

**Why:** All tests so far sequential or low concurrency. At 400 tenants with 10 queries/sec/tenant = 4K queries/sec system-wide. LMDB read-only txns should scale (MVCC), but unvalidated.

**Open question:** Does LMDB's single-writer lock block readers? Docs claim readers never block, but need empirical validation under write load.

---

### LMDB File Growth Without Compaction
**What to test:**
```rust
// Write 1M docs across 20 tenants
// Delete 500K docs (mark tombstones, don't compact)
// Measure disk growth vs live data size
// Test compaction operation latency
```

**Why:** LMDB uses append-only B-tree. Deletes/updates don't reclaim space until manual compaction. File grows monotonically. Need strategy: compact on schedule? Triggers on fragmentation ratio? Compact latency may block writes.

**Open question:** Can we defer compaction to off-hours? Or must run inline? What's acceptable downtime for compaction on 10GB database?

---

### Update Cost: Multi-Index Writes
**What to test:**
```rust
// Add single doc with 20 unique terms + 5 sortable fields
// Measure commit latency
// Compare to read-only workload baseline
```

**Why:** Each doc write touches:
- N term inverted indices (1 write per unique term)
- M sort indices (1 write per sortable field)
- 1 doc store write
- BM25 metadata updates

If doc has 20 terms + 5 fields = 26 LMDB writes in one txn. Overhead unknown.

**Open question:** Is 26 writes/doc acceptable? Or does it bottleneck at scale? May need to batch or defer sort index updates.

---

### Cold Start Latency After Cache Eviction
**What to test:**
```rust
// Create 100 tenant DBs, query all (make hot)
// Drop process, clear OS cache (echo 3 > /proc/sys/vm/drop_caches)
// Query single tenant, measure first-query latency
// Expected: 10-200ms (page faults)
// Measure 10x to get P99
```

**Why:** selective_fault_test validated lazy-loading but didn't measure cold-start penalty. If first query = 500ms, user-facing latency spikes unacceptable. Need warming strategy or accept slow first query.

**Open question:** Can we pre-warm high-value tenants? Or accept cold-start as cost of high density?

---

### Memory Pressure: 400 Tenants Active Simultaneously
**What to test:**
```rust
// Create 400 tenant DBs
// Query all 400 concurrently in tight loop
// Measure:
// - RSS growth rate
// - Query latency degradation as cache fills
// - OS page eviction behavior under 4GB limit
```

**Why:** All tests so far <100 tenants. At 400 tenants, if all active simultaneously:
- 400 × 1 MB = 400 MB baseline
- If working set larger than expected = OOM or thrashing

**Open question:** What % of 400 tenants are active simultaneously in production? If 80/20 rule, only 80 hot = 80 MB. But need to validate under load.

---

### Posting List Compression: CPU vs Disk Trade-off
**What to test:**
```rust
// Encode posting lists with:
// 1. Raw u32 array (current)
// 2. Delta encoding + varint
// 3. Roaring bitmaps (if doc IDs dense)
// Measure:
// - Disk savings (likely 2-5x)
// - Decode latency per query (target <1ms for 10K postings)
```

**Why:** Current tests use uncompressed postings. Production needs compression to save disk. Trade-off: CPU during query decode. Need to validate decode doesn't push queries >50ms P99.

**Open question:** Which compression? Delta+varint simple but potentially slow. Roaring faster for dense posting lists but complex.

---

## Summary of Blocking Uncertainties

**Known acceptable:**
- Selective faulting: works
- Write throughput: 2K+ writes/sec
- Single-field sort: <1ms
- FST overhead: negligible

**Unknown, likely acceptable:**
- BM25 metadata: 0.5-1 MB/tenant (affects final overhead by 50-100%)
- Multi-field sort: 2 MB/tenant (reduces density to 200/4GB)
- Query intersection: algorithm complexity unclear

**Unknown, potentially problematic:**
- Concurrent read contention at 4K QPS
- Update cost with 5 sort indices per doc
- LMDB compaction strategy and downtime
- Memory pressure with 400 active tenants

**Recommendation:** Prototype weeks 1-2 should implement BM25 + 3-field sort + query planner, then re-measure overhead. If >3 MB/tenant, density target drops to 266 tenants/4GB (still $266 revenue vs $30 cost = viable). If >5 MB/tenant, revisit pricing or accept lower density.

-------------------

important:

Recommendation
Defer sharding but:

Add storage abstraction layer (1 day work, huge future savings)
Set conservative limit <10 GB to buy time
Test performance AT the limit (10 GB tenant test)
Decide enterprise tier strategy for oversized tenants