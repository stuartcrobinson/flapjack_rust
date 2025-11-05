# Concurrent Read Scaling Tests (Nov 4, 8:10pm)

## Context

Tests from nov4_8pm/ validated:
- ✅ Per-env overhead: 0.067 MB (excellent)
- ✅ Sequential commits: 4ms P99 (excellent)
- ✅ Hot copy migration: 38ms for 57 MB (excellent)

**Critical unknowns remaining:**
1. Concurrent read scaling (all tests were single-threaded)
2. Memory overhead at realistic tenant sizes (1K docs is too small)

These tests validate production-scale performance.

---

## Tests

### 1. `concurrent_read_test.rs`
**Question:** Does LMDB scale to 4K+ QPS under concurrent load?

**Why critical:** All previous tests single-threaded. At 400 tenants × 10 QPS = 4K system QPS. LMDB claims "readers scale linearly" but unvalidated. Reader lock table exhaustion would kill architecture.

**Method:**
1. Create 100 tenants with 10K docs each
2. Baseline: Single-threaded queries, measure P99
3. Low concurrency: 10 threads × 100 queries
4. High concurrency: 100 threads × 50 queries = 5K QPS
5. Each thread queries random tenants

**Success criteria:**
- P99 concurrent <50ms (2x baseline acceptable)
- No reader lock table exhaustion errors
- QPS scales near-linearly with threads

**Failure modes:**
- P99 >200ms → LMDB doesn't scale to required QPS
- Errors/crashes → Reader lock table exhausted
- QPS sub-linear (10x threads = 3x QPS) → Lock contention

**Run:**
```bash
cargo run --release --bin concurrent_read_test
```

**Expected runtime:** ~30 seconds (creates 100 tenants × 10K docs)

**Key metrics to watch:**
- High concurrency P99: target <50ms
- High concurrency QPS: target >3000
- Error count: must be 0

---

### 2. `realistic_size_test.rs`
**Question:** Does memory overhead scale with corpus size?

**Why critical:** Previous test used 1K docs/tenant (tiny). Production = 10K-100K docs. If overhead scales linearly, 100x docs = 100x memory = density collapses.

**Theory:** Overhead should be constant (file descriptors + metadata). Working set should scale with *accessed* data, not total corpus.

**Method:**
1. Create 50 tenants with 100K docs each (~100 MB per tenant)
2. Measure per-env overhead
3. Query 1 tenant → measure working set
4. Query 5 tenants → measure multi-tenant working set
5. Compare to baseline (1K docs/tenant)

**Success criteria:**
- Per-env overhead <0.5 MB (ideally same as baseline: 0.067 MB)
- Working set <2 MB per active tenant
- 400 tenants @ 20% active <3.5 GB

**Failure modes:**
- Overhead scales linearly (100K docs = 100x overhead)
- Working set >5 MB per tenant
- B-tree depth or free list grows significantly

**Run:**
```bash
cargo run --release --bin realistic_size_test
```

**Expected runtime:** ~2-3 minutes (creates 50 tenants × 100K docs = 5M docs total)

**Key metrics to watch:**
- Per-env overhead: compare to baseline 0.067 MB
- Per active tenant working set: compare to baseline 0.23 MB
- Scaling ratio: should be <2x despite 100x more docs

---

## Decision Tree

```
Run concurrent_read_test
├─ P99 >200ms OR errors?
│  └─ YES → ❌ LMDB doesn't scale. Reconsider architecture.
│  └─ NO → Continue
│
Run realistic_size_test
├─ Overhead >2x baseline OR working set >5 MB?
│  └─ YES → ⚠️  Need to reduce density or tenant size limits
│  └─ NO → ✅ Architecture fully validated for production
```

---

## Expected Results

**Best case (concurrent_read_test):**
- Baseline P99: 0.5ms
- High concurrency P99: 2-5ms (4-10x degradation)
- High QPS: 4000-5000
- Errors: 0
- **Conclusion:** Linear read scaling confirmed

**Realistic case:**
- Baseline P99: 1-2ms
- High concurrency P99: 10-30ms
- High QPS: 3000-4000
- Errors: 0
- **Conclusion:** Acceptable scaling

**Failure case:**
- P99 >100ms
- QPS <2000
- Errors >0
- **Conclusion:** LMDB reader lock table or mmap contention

---

**Best case (realistic_size_test):**
- Per-env overhead: 0.067 MB (same as baseline)
- Working set: 0.5-1 MB (2-4x baseline, acceptable)
- 400 tenants @ 20% active: <500 MB
- **Conclusion:** Memory constant with corpus size

**Realistic case:**
- Per-env overhead: 0.1-0.2 MB (slight increase)
- Working set: 1-2 MB per active tenant
- 400 tenants @ 20% active: 200-500 MB
- **Conclusion:** Sub-linear scaling, acceptable

**Failure case:**
- Per-env overhead: >0.5 MB (scaling with corpus)
- Working set: >5 MB per tenant
- 400 tenants @ 20% active: >2 GB
- **Conclusion:** Need to limit tenant sizes

---

## Architecture Validation Checklist

After both tests:

✅ Per-env overhead constant (<0.5 MB)  
✅ Sequential commits fast (<15ms P99)  
✅ Hot copy migration works (<30s for 100 MB)  
⬜ Concurrent reads scale (target: <50ms P99 @ 4K QPS)  
⬜ Realistic corpus size viable (target: 100K docs <2 MB working set)

If all ✅:
- **Separate LMDB file per tenant architecture validated**
- Ready to build query planner (text + filter + sort)
- Ready to measure BM25 metadata overhead
- Ready to test global replication strategy

If any ⬜ fail:
- May need to reduce density (200 tenants/4GB instead of 400)
- May need to limit tenant size (10K docs instead of 100K)
- May need to reconsider LMDB altogether

---

## Remaining Open Questions (Not Tested Here)

1. **BM25 metadata overhead:** 
   - Doc lengths: 400 KB per 100K docs
   - Field norms: 1.2 MB per 100K docs × 3 fields
   - Term IDF: ~200 KB
   - **Total: ~1.8 MB/tenant**
   - Needs separate test

2. **Query complexity:**
   - Text search + filters + sort in one query
   - Posting list intersection performance
   - Need Roaring bitmaps or simple lookups sufficient?

3. **Write distribution:**
   - What if 50 tenants write simultaneously?
   - Queue sequentially (200ms) or commit concurrently (10ms, 2x P99)?
   - Depends on write pattern

4. **File descriptor limits:**
   - 400 envs × 2 FDs = 800 open files
   - Default ulimit = 1024
   - Need to raise to 4096+

5. **Global replication:**
   - LMDB single-writer per environment
   - Multi-region writes need forwarding or per-region DBs
   - Async replication lag acceptable?

These can be addressed after core architecture validated.