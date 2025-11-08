https://claude.ai/chat/1cfaf9ad-b765-4812-8598-98e38a99b3d0

<document>
# Phase 0 Test Results Summary

## Test 1: Faceting Viability
**Question:** Can we aggregate facets at query time or need pre-built indices?

**Results:**
- 1K results × 5 facet fields: P99 = 2.07ms
- 10K results × 5 facet fields: P99 = 18.21ms

**Conclusion:** Query-time aggregation viable. No schema changes needed.

**Limitations:** Only tested STRING and u64 facets. Didn't test hierarchical facets, multi-valued fields, or correctness edge cases (missing values, unicode). Assume Tantivy's facet API if available, but fallback to manual iteration validated.

---

## Test 2: Query Planner Threshold
**Question:** When to use filter-first vs text-first execution?

**Results:**
| Filter Returns | Filter Cost | Text Cost | Winner |
|----------------|-------------|-----------|--------|
| 612 docs       | 0.06ms      | 0.17ms    | Filter |
| 1761 docs      | 0.07ms      | 0.08ms    | Filter |
| 3631 docs      | 0.11ms      | 0.08ms    | Text   |

**Cost model:**
- Filter: 0.06ms + N×0.000016ms (linear growth)
- Text search: ~0.08ms (constant - early termination works)
- Crossover: ~1250 docs

**Conclusion:** Set PLANNER_THRESHOLD = 1200 docs

**Limitations:** 
- Single-term query only. Phrase queries, fuzzy matches, multi-field queries not tested.
- Tantivy's combined BooleanQuery may optimize differently than manual filter-first.
- Filter on numeric range (indexed FAST field). String filters, date ranges, geo-filters untested.
- Scoring cost assumed negligible - didn't validate BM25 computation on large filtered sets.

---

## Test 3: Fast Field Memory (In-RAM)
**Question:** Do sortable fields blow memory budget?

**Results (100K docs, in-RAM index):**
- Minimal schema: 12.3 MB
- 5 fast fields: 41.2 MB
- 5 fast + STORED: 54.8 MB

**Scaling:** 10K→100K = 1.7x growth (not 10x), indicating high fixed overhead.

**Extrapolated:** 50K doc tenant ≈ 31 MB

**Conclusion:** In-RAM indices too expensive - would limit to 130 tenants/4GB node.

**Limitations:**
- In-RAM != disk-backed. mmap behaves differently.
- Measured RSS during indexing (hot write buffers), not steady query load.
- "After drop" showed 20 MB (vs 41 MB active) - unclear if steady-state.

---

## Test 4: Realistic Density (Disk-Backed)
**Question:** Actual memory under production load patterns?

**Setup:**
- 150 tenants × 50K docs (7.5M docs total)
- Disk-backed indices with mmap
- Zipf distribution: 80% queries → top 20% tenants
- 546K queries over 30s

**Results:**
- Initial RSS after indexing: 317 MB (+314 MB from baseline)
- Steady-state after warmup: 360 MB
- **Per-tenant: 2.38 MB**

**Capacity:**
- 4 GB node: 1721 tenants (theoretical max)
- Safe target: 600 tenants (60% utilization)

**Conclusion:** Original 400 tenant target exceeded by 1.5x.

**Limitations:**
- Only tested read load. Didn't simulate concurrent writes, segment merges, or tenant migrations.
- Zipf distribution may not match real workload. If uniform (all tenants equally active), working set grows.
- Query complexity: simple single-term search. Complex queries (filters, sorts, facets) may increase per-query RSS.
- 30s duration may miss long-tail memory leaks or gradual RSS growth.
- Single node test. Multi-node with network replication untested.

---

## Test 5: Filter Isolation
**Question:** Actual cost breakdown of filter execution?

**Results:**
- Filter alone: 0.06-0.22ms (scales with cardinality)
- Text search alone: 0.08ms (constant)
- Scoring 20 docs: 0.04-0.18ms (scales with cardinality fetched)

**Validates:** Filter-first wins below ~1250 docs, text-first wins above.

**Limitations:**
- "Scoring" was fake (just fetch docs). Didn't actually run BM25 scorer on filtered subset.
- Single filter type (numeric range). Multi-clause filters (AND/OR) untested.
- Top-20 retrieval. Large result limits (1000 docs) may shift crossover.

---

# What This Means for Build Plan

## Blocking Issues: RESOLVED

1. **Faceting architecture:** Query-time aggregation works. No index schema changes required. Phase 1.5 can proceed as planned.

2. **Query planner threshold:** 1200 docs validated. Implement heuristic in Phase 1.3. No need for cost-based optimizer at MVP.

3. **Density target:** 600 tenants/4GB node viable (1.5x better than plan). Replication model still works (6.8 cores @ 600 tenants vs 4.5 cores @ 400 tenants = acceptable).

## Deferred Risks (Still Unknown)

### High Priority (Blocks Phase 2-3)

**1. Concurrent write latency**
- Tested: sequential commits (0.28ms P99)
- Unknown: 40 tenants writing simultaneously
- Risk: fsync contention, lock contention → P99 >50ms
- Gate: Phase 2.2 (batched writes) must validate this

**2. Update/delete behavior**
- Tested: insert-only workload
- Unknown: segment fragmentation under updates, merge CPU cost
- Risk: background merge blocks queries or spikes latency
- Gate: Phase 3 migration tests should include update-heavy workload

**3. Batching accumulation rate**
- Tested: pre-batched commits (10 commits arrive together)
- Unknown: random arrival pattern (400 tenants × 2.5 writes/sec = Poisson arrival)
- Risk: takes 5-10s to accumulate batch → replication lag >4s target
- Mitigation: cross-tenant batching (commit when ANY 10 tenants have writes) - adds routing complexity

### Medium Priority (Validate in Phase 4)

**4. Query distribution uniformity**
- Tested: Zipf (80/20 rule)
- Unknown: what if enterprise customer queries ALL their data uniformly?
- Risk: working set = 600×30 MB = 18 GB (not 360 MB), node overloaded
- Mitigation: monitor per-tenant query rates, migrate hot tenants to dedicated nodes

**5. Query complexity impact**
- Tested: simple single-term search
- Unknown: combined filter+sort+facet+phrase query
- Risk: complex query touches more mmap pages → RSS spike
- Validate: end-to-end integration tests with realistic query mix

**6. Multi-node coordination**
- Tested: single node
- Unknown: distributed tenant router, leader election, split-brain scenarios
- Risk: migration races, double-writes, data loss
- Gate: Phase 4.1 (etcd/Consul integration)

### Low Priority (Optimize Later)

**7. Faceting correctness**
- Tested: performance only
- Unknown: edge cases (multi-valued fields, nulls, unicode)
- Risk: wrong counts returned to user
- Validate: property-based tests in Phase 1.5

**8. Schema evolution**
- Not tested
- Unknown: how to add field to existing tenant without reindex
- Risk: downtime during schema changes
- Design: defer to Phase 5+ (not MVP blocker)

## Build Plan Adjustments

### Phase 1: NO CHANGES
- Threshold = 1200 (validated)
- Query-time faceting (validated)
- Single-tenant foundation unchanged

### Phase 2: ADD CONCURRENT WRITE TEST
Insert before Phase 2.3 (HTTP API):
```
Phase 2.2b: Concurrent Write Validation
- 40 threads × random tenant × 10 writes each
- Measure commit P99 latency
- Pass condition: P99 <50ms
- If fail: need write queue with background commit thread
```

### Phase 3: ADD UPDATE/MERGE TEST
Insert in Phase 3.1:
```
Phase 3.1b: Update/Delete Under Load
- Index 50K docs → update 25K docs (simulate price changes)
- Measure: merge CPU %, query P99 during merge
- Pass condition: P99 <2x baseline during merge
- If fail: tune merge policy or separate merge nodes
```

### Phase 4: ADD UNIFORM QUERY TEST
Insert in Phase 4.2:
```
Phase 4.2b: Uniform Query Distribution
- 600 tenants, query ALL tenants uniformly (not Zipf)
- Measure: RSS growth, query P99 stability
- Pass condition: RSS <3.5 GB, P99 <100ms
- If fail: reduce density to 400 tenants or add query result cache
```

### Density Assumption: INCREASE
- Original: 400 tenants/node
- Validated: 600 tenants/node
- Impact: 
  - Replication CPU: 4.5 → 6.8 cores (acceptable)
  - Batching: easier (more write arrival density)
  - Cost: 1.5x fewer nodes for same tenant count

## Critical Assumptions Remaining

### Assumption 1: Zipf distribution holds
**If wrong:** Uniform distribution blows working set to 18 GB. Need 16 GB nodes or 200 tenant density.

**Validation strategy:** Monitor in production. If per-tenant query rate variance <2x (not 80/20), trigger rebalancing or caching.

### Assumption 2: Merge doesn't block
**If wrong:** Tantivy background merge holds IndexWriter lock, blocking commits. P99 spikes to seconds during merge.

**Mitigation:** Test in Phase 3. If blocked, use separate merge thread with atomic segment swap.

### Assumption 3: Replication lag <4s acceptable
**If wrong:** Customers expect read-after-write consistency globally. 4s lag = bad UX.

**Redesign trigger:** Need sync replication (write waits for N replicas) or sticky routing (read from primary).

### Assumption 4: Single-writer per tenant
**If wrong:** Customer has multiple apps writing to same tenant concurrently. Need distributed write coordination.

**Complexity explosion:** Requires consensus (Raft/Paxos) per tenant. Out of scope for MVP. Document as limitation.

## Go/No-Go Decision

**GO.** All MVP-blocking questions answered. Remaining risks are:
1. Detectable in later phases (concurrent writes, updates)
2. Mitigatable (merge tuning, density adjustment)
3. Acceptable for launch (replication lag, single-writer limit)

**Start Phase 1.1 immediately.** No architectural redesign needed.

## Expected Failure Modes (Plan for These)

**Most likely to break assumptions:**

1. **Enterprise customer with 1M doc tenant** - Our tests maxed at 100K docs. Extrapolation may not hold. 1M doc tenant could be 150 MB (linear scaling) or 500 MB (overhead dominates). Monitor in beta.

2. **Hot tenant monopolizes node** - One tenant gets featured on Reddit, 10K QPS spike. Other tenants on same node starve. Need per-tenant rate limiting AND fast migration (your 38ms copy helps here).

3. **Batching doesn't accumulate** - Real workload is bursty (all tenants write at :00, :15, :30, :45 for scheduled jobs). Batch size = 1-2 commits most of the time, not 10. Replication cost blows up to 20 cores. Mitigation: cross-tenant batching or time-based batching (commit every 1s regardless).

**None of these kill the project. They just force optimization earlier than planned.**

---

**Bottom line:** Tests validated core feasibility. Unknowns remain but are manageable. Phase 1 can start with high confidence. Budget 20% schedule slip for deferred unknowns discovered during Phase 2-4 integration.
</document>