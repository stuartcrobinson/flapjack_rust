https://claude.ai/chat/a2941566-997d-4b81-85fa-2079115fea24

## Results Analysis

**Test #2: Sequential commits**
- 20 commits: **81ms total** (4ms avg per tenant)
- P99: **4.18ms**
- Concurrent penalty: **2.1x P99** (8.58ms vs 4.18ms)

**Unexpected finding:** Concurrent commits were *faster* total time (11ms vs 81ms) but higher P99. This contradicts your Tantivy test where concurrent commits caused massive serialization (3,851ms).

**Why the difference?**
1. **Small write batch:** 50 writes/commit is trivial. Fsync dominates, actual write work minimal.
2. **Parallel fsync worked:** OS didn't serialize as harshly as with Tantivy. Could be:
   - Different filesystem/kernel (what are you running?)
   - LMDB's write pattern more fsync-friendly
   - Separate LMDB files = separate inodes = less lock contention than Tantivy's separate directories

**Critical implication:** Sequential batching may not be necessary. If concurrent commits only add 2x P99 penalty (4ms → 8ms), both are well under 1-sec visibility target. You could commit all tenants concurrently and save 70ms.

---

## Architecture Status

All three tests passed decisively:

| Metric | Target | Actual | Status |
|--------|--------|--------|--------|
| Per-env overhead | <1 MB | 0.067 MB | ✅ 15x better |
| Active working set | <2 MB | 0.23 MB | ✅ 9x better |
| Sequential commit P99 | <15ms | 4.18ms | ✅ 3.5x better |
| Migration copy time | <30s | 0.04s | ✅ 750x better |

**Separate-file-per-tenant architecture is validated.**

---

## Unanswered critical questions

### 1. **Concurrent read scaling** (highest priority)
Tests were all single-threaded queries. At 4K QPS system-wide:
- Does LMDB's reader lock table become bottleneck?
- Does mmap page fault rate spike under high concurrency?
- P99 query latency degradation?

**Test needed:**
```rust
// 100 threads × 40 QPS each = 4K QPS
// Each thread queries random tenants
// Measure P99 latency vs single-threaded baseline
```

This is **blocking** because if LMDB can't handle concurrent reads, entire architecture fails at scale regardless of low memory overhead.

### 2. **Realistic tenant size overhead**
Test #1 used 1K docs per tenant (tiny). At 100K docs:
- Does B-tree depth affect per-env overhead?
- Does free page list grow significantly?
- Working set still 0.23 MB or scales with corpus size?

**Quick validation:**
```rust
// Modify Test #1: docs_per_tenant = 100_000
// Re-run, compare overhead
```

### 3. **BM25 metadata overhead**
Current tests have no search structures. Production needs:
- Doc lengths: 4 bytes × doc_count
- Field norms: 4 bytes × doc_count × field_count
- Term IDF: 4 bytes × unique_terms

For 100K docs, 3 fields, 50K unique terms:
- Doc lengths: 400 KB
- Field norms: 1.2 MB
- Term IDF: 200 KB
- **Total: ~1.8 MB/tenant**

Combined with 0.23 MB working set = **2 MB/tenant active**. At 80 active tenants (20%): **160 MB**. Still fits 4GB but reduces headroom significantly.

### 4. **Write distribution assumptions**
Test #2 assumes you can batch by accumulating over 1 second. But:
- What if 50 tenants all get writes in same 10ms window?
- Do you queue them (sequential, 50 × 4ms = 200ms last-tenant latency)?
- Or commit concurrently (11ms total, 8.58ms P99, acceptable)?

Concurrent might be better if write spikes are common.

---

## Recommended next steps (priority order)

**1. Concurrent read test** (write this next)
Test 100 threads doing read-only queries across random tenants at 4K QPS. If this fails (P99 >100ms or crashes), architecture needs major revision.

**2. Realistic corpus size**
Re-run Test #1 with 100K docs per tenant. Validate overhead doesn't scale with corpus size.

**3. Move to query planner**
You've validated storage layer. Now build:
- Text search (FST → posting lists)
- Filter intersection (price ranges, etc.)
- Sort (integer key indexes)
- BM25 scoring

Tests #1-3 confirm LMDB is viable. But you haven't validated that **search quality + performance** meets Algolia/Meilisearch parity.

---

## One concern about Test #2 results

Concurrent commits showed **0x penalty** in total time (11ms concurrent vs 81ms sequential). This suggests fsync parallelization worked. But your Tantivy test showed catastrophic serialization.

**Hypothesis:** Tantivy's 3,851ms wasn't fsync serialization - it was lock contention in Tantivy's index writer. LMDB doesn't have that (separate files = no shared locks).

**Implication:** Your original motivation ("cross-tenant atomic commits to avoid fsync serialization") was solving the wrong problem. The real issue was Tantivy's architecture, not fsync.

**Test to validate:** Run Tantivy test again but with separate index directories on separate mount points (if possible). If still slow, it's Tantivy internals, not fsync.

But this is academic now - LMDB separate files clearly work. Move forward with concurrent read test.