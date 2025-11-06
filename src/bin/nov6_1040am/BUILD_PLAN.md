https://claude.ai/chat/a53df619-c47f-42bb-9b07-d7f13dec5480

# Flapjack Search Engine: Build Plan

## Requirements Summary
- **Multi-tenant from day 1:** Isolated per-tenant indices, no cross-contamination
- **Seamless migration:** Move tenants between nodes without downtime
- **Global replication:** Async segment sync to N replicas (eventual consistency acceptable)
- **Feature parity:** Text search, filters, facets, sorting, BM25 ranking
- **Scale target:** 100-500 tenants/node, 10K-100K docs/tenant

## Architecture Decisions
- **Storage:** Tantivy (segment-based, validated at 4.10 MB/tenant RAM)
- **Replication:** WireGuard + rsync batch (4.5 cores for 30 replicas)
- **Write model:** Single-writer primary per tenant, async replicas
- **Isolation:** Soft (shared process, monitor resource usage)

---

## Critical Abstraction Layers

### 1. Storage Interface
**Purpose:** Swap Tantivy for alternative later without rewriting application logic.

**Boundary:** `TenantStorage` trait exposing document write, segment read, delete operations. Hide Tantivy-specific IndexWriter/IndexReader behind this.

**Why abstract:** If memory becomes bottleneck, could move cold tenants to LMDB or S3-backed storage without touching API layer.

---

### 2. Replication Transport
**Purpose:** Decouple segment transfer from application logic.

**Boundary:** `ReplicationTransport` trait for sending/receiving segment files. Current impl = WireGuard+rsync. Future could be QUIC, direct TCP streams, or S3 pull.

**Why abstract:** Network bandwidth may become bottleneck. Testing showed 0.65 MB/s (underutilized). May need protocol optimization or CDN-based distribution.

---

### 3. Tenant Router
**Purpose:** Resolve tenant → node mapping, coordinate migrations.

**Boundary:** `TenantRouter` trait returning node address for given tenant. Initially in-memory HashMap. Later distributed store (etcd/Consul) for multi-node orchestration.

**Why abstract:** Single-node MVP doesn't need coordination, but multi-node requires distributed consensus. Clean boundary prevents rewriting routing logic.

---

### 4. Query Planner
**Purpose:** Choose execution strategy (filter-first vs text-first based on selectivity).

**Boundary:** `QueryPlanner` trait taking query + tenant metadata, returning execution plan. Initially simple heuristic (<1200 docs = filter-first). Later could be cost-based or ML.

**Why abstract:** Current tests validated simple threshold, but large-scale production may need adaptive planning based on actual cardinality statistics.

---

**Do NOT abstract:** BM25 implementation, tokenization, segment merging. These are Tantivy internals. Swapping requires rewriting search engine.

---

## Build Phases

### Phase 1: Single-Tenant Foundation (Week 1-2)

**1.1 Document Indexing**
- Tantivy schema definition
- Add/commit document operations
- **Test:** Index 10K docs, commit P99 <50ms

**1.2 Text Search (BM25)**
- Basic query parsing
- BM25 scoring via Tantivy
- **Test:** Query P99 <10ms on 10K corpus, correct ranking

**1.3 Filters (Numeric/Date Ranges)**
- Filter definition (field + range)
- Intersection with text results
- **Test:** Combined query P99 <30ms, correct filtering

**1.4 Sorting**
- Sort by field (ascending/descending)
- Maintain relevance sort option
- **Test:** Sorted query P99 <50ms, correct ordering

**Validation gate:** Single-tenant search works end-to-end with acceptable latency.

---

### Phase 2: Multi-Tenant, Single-Node (Week 3-4)

**2.1 Tenant Isolation**
- Per-tenant Tantivy index directories
- Tenant ID routing to correct index
- **Test:** 50 tenants × 1K docs, no cross-tenant leakage, memory <200MB

**2.2 Batched Writes**
- Write queue per tenant
- Background thread commits every 100ms or 100 docs
- **Test:** 20 tenants concurrent writes, P99 commit <50ms, all docs queryable after flush

**2.3 HTTP API**
- REST endpoints: POST /documents, GET /search
- Tenant routing via path parameter
- **Test:** End-to-end HTTP request → indexed → searchable in <200ms

**Validation gate:** Multi-tenant works on single node with acceptable isolation and performance.

---

### Phase 3: Migration & Replication (Week 5-6)

**3.1 Tenant Export/Import**
- Serialize tenant index to archive
- Deserialize on target node
- **Test:** Export 100K docs, import on new node, query results identical, <5s for 50MB tenant

**3.2 Segment Sync (Primary → Replica)**
- Detect new segments after commit
- Batch segments across tenants (10 commits)
- rsync over WireGuard to replica
- **Test:** Replicate 10 tenants × 1K docs, replica lag P99 <2s, query results match primary

**3.3 Routing Layer Migration**
- Update tenant → node mapping
- Dual-read during migration (query both nodes, accept either)
- **Test:** Migrate tenant under load, <1s unavailability, no lost writes

**Validation gate:** Can move tenants between nodes without data loss or extended downtime.

---

### Phase 4: Production Readiness (Week 7-8)

**4.1 Multi-Node Coordination**
- Distributed tenant router (etcd/Consul)
- Leader election for migration orchestration
- **Test:** 3 nodes, simulate node failure, tenants auto-rebalance

**4.2 Monitoring & Resource Limits**
- Per-tenant memory/CPU tracking
- Alert on threshold breach (noisy neighbor detection)
- **Test:** Tenant A saturates CPU, alert triggers, no impact on Tenant B queries

**4.3 API Authentication & Rate Limiting**
- Tenant API keys
- Per-tenant QPS limits
- **Test:** Tenant A exceeds limit, gets 429, Tenant B unaffected

**Validation gate:** Can operate multi-tenant cluster with monitoring and protection against abuse.

---

### Phase 5: Feature Parity (Week 9-12)

**5.1 Faceting**
- Aggregate over field values during query
- Return counts per facet value
- **Test:** Facet query on 100K docs, P99 <100ms, correct counts

**5.2 Highlighting**
- Mark query terms in result snippets
- Return contextual excerpts
- **Test:** Highlight query "laptop computer", returns snippets with terms marked

**5.3 Typo Tolerance**
- Levenshtein distance fuzzy matching
- Auto-correct common misspellings
- **Test:** Query "laptp" returns "laptop" results

**5.4 Custom Ranking**
- User-defined boost factors per field
- Combine with BM25 score
- **Test:** Boost "title" 2x, verify ranking changes

**Validation gate:** Feature set competitive with Meilisearch/Algolia.

---

## Testing Strategy

### Unit Tests
- Per-function correctness (indexing, search, filters)
- Run on every commit
- Target: >80% code coverage

### Integration Tests
- End-to-end flows (HTTP API → index → search)
- Multi-tenant isolation validation
- Run before merge to main

### Performance Tests
- Latency benchmarks (P50/P99 on 10K, 100K, 1M docs)
- Memory profiling (50 tenants, track RSS growth)
- Run weekly, alert on regression

### Chaos Tests
- Kill nodes during migration
- Saturate CPU with noisy tenant
- Verify graceful degradation
- Run before production deploy

---

## Open Risks

**Query throughput under concurrent load:** Tests measured sequential queries. Need validation at 4K QPS system-wide (100 threads × 40 QPS). If P99 >100ms, may need query caching or connection pooling.

**Concurrent write memory spikes:** Tests showed 4.10 MB/tenant sequential. If 40 tenants write simultaneously (10% of 400), transient spike may reach 160MB. Monitor in staging.

**Batching accumulation rate:** Tests assumed 10 commits arrive simultaneously. Real workload: 400 tenants × 2.5 writes/sec = random arrival. May take 3-5s to accumulate batch, increasing lag. May need cross-tenant batching (adds routing complexity).

**Global replication at 30 replicas:** 4.5 cores validated but assumes all replicas need same freshness. Consider tiered replication (5 hot replicas <1s lag, 25 cold replicas <30s lag) to reduce cost.

**Segment merge CPU:** Tantivy merges segments in background. Not measured under sustained write load. Could spike CPU during merge, affecting query latency.

---

## Decision Gates

**After Phase 1:** If single-tenant search fails performance targets, architecture is wrong. Abort and reconsider.

**After Phase 2:** If multi-tenant isolation breaks (cross-tenant leakage or noisy neighbor impact >10% latency degradation), need hard isolation (containers/processes).

**After Phase 3:** If migration causes >5s downtime or data loss in testing, need write-ahead log or dual-write strategy.

**After Phase 4:** If monitoring shows memory growth >5 MB/tenant under load, reduce density target to 200 tenants/node.

**Before Phase 5:** Validate market demand for features. If 80% of beta customers don't use facets, defer and focus on performance/reliability.



# Overall Assessment

**Architecture is sound.** Validated through testing, not speculation. Abstractions are at correct boundaries (storage, transport, routing). Phases build incrementally with testable gates.

**Phase ordering has issues.**

---

# Critical Problems

## 1. Faceting Deferred Too Late

**Risk:** Faceting affects index structure fundamentally.

Facets require aggregating field values across result set. Two approaches:

**A) Query-time aggregation:**
- Iterate results, count unique values
- Works for <10K results, breaks at scale
- No index changes needed

**B) Pre-built aggregation index:**
- Separate data structure (like Lucene's SortedSetDocValues)
- Updates on every document write
- Adds ~0.5-1 MB/tenant memory

If you defer to Phase 5 and realize you need (B), you're rebuilding indexing pipeline. **Test assumption now:** Can you aggregate 100K results in <100ms with query-time approach? If no, faceting is Phase 1 work, not Phase 5.

**Hypothesis to validate:** Most facet queries have selective filters (e.g., "laptops under $1000" → maybe 500 results → aggregate those). If true, query-time works. If users want "aggregate all 1M products by category," you need pre-built indices.

---

## 2. Sort Implementation Underspecified

Your tests showed single-field sort works (<1ms). Production needs:

**Primary sort + secondary tiebreaker:** "Sort by price ascending, then by date descending for ties."

**Approaches:**

**A) Tantivy's fast fields (doc values):**
- Store sort keys per document
- Load top-K sort keys at query time
- Memory: ~4 bytes × num_docs per sortable field
- Your test: 0.12ms for range scan suggests this works

**B) Separate sort index (your earlier LMDB tests):**
- B-tree: sort_key → doc_id
- Range scan returns sorted doc IDs
- Then fetch docs

Your Tantivy tests used (A). Fast fields are standard. **Assume this is the path.** Multi-field sort = load multiple fast fields, compare tuples.

**Unanswered:** Does loading 5 fast fields × 100 docs at query time blow P99 latency? Estimate: 5 fields × 4 bytes × 100 docs = 2KB read. Should be <1ms. **Validate in Phase 1.4 by testing 5-field sort, not just single field.**

---

## 3. Filter-First vs Text-First Query Planning

Your earlier tests showed crossover at ~1200 docs:
- Filter returns <1200 docs → filter-first wins (score subset)
- Filter returns >1200 docs → text-first wins (early termination)

**This is Phase 1.3 work, not Phase 4.** Query planner threshold is testable immediately. Don't defer.

Extend Phase 1.3:
```
Test: Query "laptop" + price:[500-2000]
- If filter = 500 docs: Assert filter-first chosen, P99 <5ms
- If filter = 5000 docs: Assert text-first chosen, P99 <30ms
```

---

## 4. Batching Strategy Unvalidated

Your tests assumed 10 commits accumulate simultaneously. Phase 2.2 says "commit every 100ms or 100 docs."

**Problem:** At 400 tenants × 2.5 writes/sec average = random arrival. Some tenants get 1 write in 100ms, others get 50. Your batch size varies wildly.

**Two strategies:**

**A) Per-tenant batching:**
- Each tenant commits when reaches 100 docs OR 100ms timeout
- 400 separate timers
- Replication CPU varies: sometimes 0.015s/commit (full batch), sometimes 0.150s/commit (1 doc)

**B) Global batching (cross-tenant):**
- Accumulate writes across ALL tenants
- Commit when global queue reaches 1000 docs OR 100ms
- Fixed replication cost
- But: single commit touches 100+ tenant indices (does Tantivy handle this?)

Your tests didn't validate (B). If (A), your 4.5-core projection is optimistic (assumes full batches). Real cost likely 10-15 cores.

**Decision needed in Phase 2.2:** Which strategy? Test both, measure actual CPU cost under random write arrival.

---

## 5. Missing: Update/Delete Handling

Tantivy segments are immutable. Update = delete + reindex. Delete = mark tombstone, space reclaimed on merge.

**Implications:**

**Update-heavy workload:** Segment fragmentation grows. Merge CPU spikes. Your tests indexed fresh, never updated.

**Delete-heavy workload:** Disk grows until merge. 1M docs → delete 500K → still 1M disk usage until compaction.

**Phase 3 should include update/delete tests:**
- Index 100K docs
- Update 50K docs (simulate price changes)
- Measure: merge CPU spike, query latency during merge, disk growth

If merge blocks writes (some engines do), your <50ms commit P99 breaks. Tantivy claims background merge, but validate under load.

---

## 6. Memory Projection Assumes Sequential Access

Your 4.10 MB/tenant was measured with sequential query pattern. Phase 4 needs concurrent load test:

**100 threads × random tenant queries × 40 QPS:**
- Does mmap thrash (page fault storm)?
- Does RSS grow to 400 × 4.10 MB = 1.6 GB (all tenants hot)?
- Or does LRU cache keep it at 80 × 4.10 MB = 328 MB (20% active)?

Your density assumption (400 tenants/4GB) relies on "most tenants idle." **If query distribution is uniform (all 400 active), density drops to 1000/4GB.**

**Phase 4 must test:** Zipf distribution (realistic: 80/20 rule) vs uniform (worst case). Adjust density target based on actual working set.

---

# Revised Phase Order

**Phase 1:** Add multi-field sort test and filter selectivity threshold to 1.3/1.4. Don't proceed to Phase 2 without validating these.

**Phase 1.5 (new):** Faceting prototype. Test query-time aggregation on 10K results. If P99 >50ms, you need pre-built indices = Phase 2 rework.

**Phase 2:** Add cross-tenant vs per-tenant batching comparison. Measure actual replication CPU cost under random arrival. Your 4.5-core projection is at risk.

**Phase 3:** Add update/delete/merge test. Background merge CPU cannot block writes or spike query latency >2x.

**Phase 4:** Concurrent load test with realistic query distribution. Validate 400 tenants/node or revise to 200.

**Phase 5:** (Remains same, but may be mostly done if Phase 1.5 forced pre-built facet indices.)

---

# What's Missing Entirely

**Write-ahead log:** Your migration test showed 38ms copy works. But what if node crashes during commit? Tantivy durability model unclear. Does fsync guarantee prevent data loss? Test: kill -9 during write, verify no corruption.

**Replica consistency check:** Async replication means replicas lag. How do you detect replica divergence (e.g., segment corruption, missed sync)? Need periodic checksum validation.

**Rate limiting placement:** Phase 4.3 says "API rate limiting." But should be at ingress (reverse proxy) not application. Otherwise malicious tenant can DoS by saturating connection pool before rate limit applies.

**Cost monitoring per tenant:** You measure memory/CPU but don't mention cost attribution. If Tenant A uses 10x CPU of average, do you charge more? Or just migrate them? Define policy now, affects monitoring design.

---

# Core Design Blind Spots

**Replication lag monitoring:** You validated 1-4s lag in tests. Production needs visibility: "What's current lag per replica?" If lag spikes to 60s, is that failure or acceptable? Define SLO now.

**Schema evolution:** Customer adds new field. Do you reindex all docs? Or schema-on-read? Meilisearch does automatic schema inference. You need strategy.

**Backup/restore:** Migration handles live tenants. What about disaster recovery? S3 snapshots? How often? Restore time SLA?

**Query cache:** Your P99 is <50ms cold. With cache, could be <5ms. But cache invalidation on write? Per-tenant cache size? Defer or build in Phase 2?

---

# Answer: Feel Good?

**No.** Three blocking issues:

1. **Faceting deferred too late** (may require index rework)
2. **Batching strategy untested** (4.5-core projection at risk)
3. **Update/delete/merge untested** (could break latency guarantees)

Fix: Test faceting aggregation performance in Phase 1. Add batching strategy comparison in Phase 2. Add update/merge test in Phase 3. Then feel good.

**Start building Phase 1 now.** 

Core unknowns (faceting architecture, batching strategy, update/merge CPU) won't block basic indexing → search pipeline. You'll discover them naturally when implementing.

**One critical gap:** Query planner threshold (1200 docs) came from LMDB tests with in-memory data structures. **Validate this holds with Tantivy B-tree scans** by running your filter selectivity test from `nov4_1100pm/FINAL_RESULTS.md` but on actual Tantivy indices, not LMDB. Takes 2 hours. If crossover point shifts significantly (e.g., 5000 docs), query planner logic changes.

Otherwise: build, measure, iterate. Testing without production context has diminishing returns.