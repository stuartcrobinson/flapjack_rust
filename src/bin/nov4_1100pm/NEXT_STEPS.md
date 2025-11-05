## Critical Unknowns

### 1. **Combined Query Latency (NOT TESTED)**

Your tests isolated components. Real queries combine:
- BM25 text search: 0.4ms (your earlier test)
- Filter range query: 0.2-0.9ms (just tested)
- **Sort field lookup: UNKNOWN**
- Intersection: 0.001ms (Test 2)

**Risk:** Sort index adds 5-10ms, blows 30ms P99 budget.

**Test:** Combined query with filter + sort on single field.
- `"laptop" AND price:[500-2000] ORDER BY date DESC`
- Measure: Load 100 sort keys for top-100 results
- Target: Combined <30ms P99

### 2. **Write Path with Filters (NOT TESTED)**

Test 1 measured batch commits. Didn't validate:
- **Update correctness:** Modify doc price 500→1000, does old filter entry get deleted?
- **Delete handling:** Remove doc, does it disappear from all 6 indices?
- **Concurrent read during write:** Does MVCC actually work or do reads block?

**Risk:** Write logic broken, stale filter entries accumulate, queries return deleted docs.

**Test:** 
- Index 1000 docs
- Update 500 docs (change filter field values)
- Delete 250 docs
- Query during concurrent writes
- Validate: No stale entries, reads never block

### 3. **Multi-Tenant Write Contention (NOT TESTED)**

Test 1 was single tenant. Multi-tenant unknowns:
- 20 tenants × 100 writes/sec = 2000 writes/sec aggregate
- Each tenant: separate LMDB file, independent commits
- **Do OS fsyncs serialize across files?** (probably yes on single disk)

**Risk:** 20 tenants on same disk = fsync serialization = 20x slower commits.

**Test:**
- Create 20 tenant LMDB environments
- Concurrent batch writes across tenants
- Measure: Does P99 degrade linearly? (bad) or stay constant? (good)

### 4. **Memory Under Load (NOT MEASURED)**

You validated 2.3MB/tenant working set for reads. Write tests didn't check:
- RSS during 100-doc batch commit
- B-tree rebalancing spikes
- 20 tenants × concurrent writes

**Risk:** Memory spike >100MB during writes, OOM at 400 tenants.

**Test:** Monitor RSS during load test (20 tenants, concurrent writes).

### 5. **Filter Index Memory (ESTIMATED, NOT MEASURED)**

Assumed: 0.4MB per filter index = 2MB for 5 indices. Based on what?

**Risk:** Actual memory 2-5x higher, invalidates 400 tenant density.

**Test:** Measure actual working set with 5 filter indices populated (10K docs, realistic distributions).

---

## Priority Ranking

**1. Combined query test** (blocks query path design)
- Answers: Can we hit 30ms P99 combined?
- Takes: 30 minutes to write, 2 minutes to run
- Decision: If fails, reduce filter count or simplify

**2. Write path correctness** (blocks anything production)
- Answers: Does update/delete work?
- Takes: 1 hour to write, 5 minutes to run
- Decision: If broken, debug before continuing

**3. Multi-tenant write contention** (validates cost model)
- Answers: Does fsync serialize across files?
- Takes: 1 hour to write, 10 minutes to run
- Decision: If serializes, need SSD or split across disks

**4. Memory under load** (validates 400 tenant target)
- Answers: Actual RSS during production load
- Takes: 30 minutes, piggyback on test 3
- Decision: If >4GB at 20 tenants, max density drops

**5. Filter memory validation** (refines capacity model)
- Answers: True working set with filters
- Takes: 20 minutes
- Decision: Adjust tenant density calculation

---

## What I'd Do Next

**Write test 1 (combined query).** It's the most blocking unknown. If sort index kills latency, the entire filter approach needs rethinking.

**Then test 2 (write correctness).** You can't proceed without knowing updates work.

**Then test 3 (multi-tenant).** If fsync serializes, your cost model is wrong by 20x.

Tests 4-5 are refinements, not blockers. Do after the critical path is clear.

---

## What You Haven't Considered

**Faceting cost:** Users expect `"laptop" → show facets: price ranges, brands, ratings`. This requires:
- Aggregate across ALL matching docs (not just top-100)
- Multiple filter intersections simultaneously
- Could be 10-50ms added latency

**Didn't see faceting in your tests.** Is it out of scope?