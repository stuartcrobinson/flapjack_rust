https://claude.ai/chat/46629a02-dc90-4489-9e75-96b8dde01623
## 🎯 Results Analysis

### Write Amplification Test: ⚠️ **MIXED RESULTS**

**The Bad:**
- ❌ Single-doc commit: 42ms P99 (target: <15ms) - **3x over budget**
- This is 6 B-trees × LMDB's 2 fsyncs = unacceptable for real-time

**The Good:**
- ✅ Batch(50): 0.31ms/doc, 40ms P99 commit
- ✅ Batch(100): 0.16ms/doc, 32ms P99 commit  
- ✅ Batch(500): 0.045ms/doc, 26ms P99 commit

**Sweet spot: Batch size 100-500**
- 6,200-22,000 docs/sec throughput
- <35ms P99 commit latency
- Acceptable for near-real-time indexing

### Top-K Retrieval Test: ✅ **PERFECT**

- Text-first: 0.001ms P99 in all common scenarios (100x faster than target!)
- Early termination: Stops at exactly 100 checks (perfect efficiency)
- Filter-first: **NEVER wins** except ultra-selective filter (tie at 0.14ms)
- No need for Roaring bitmaps or WAND

---

## 🚨 Critical Architecture Decisions

### 1. **You CANNOT support single-document writes**

42ms P99 kills real-time indexing. Your options:

**Option A: Batch-only writes (RECOMMENDED)**
```rust
// API design: Accumulate writes, commit every 100ms or 50 docs
index.queue_document(doc); // Returns immediately
// Background thread commits batches
```
- Users see ~100ms write latency (acceptable for search)
- Throughput: 6,200+ docs/sec
- Implementation: Simple background thread

**Option B: Enable LMDB NOSYNC mode**
```rust
EnvOpenOptions::new()
    .flags(EnvFlags::NO_SYNC) // Skip fsync
```
- Single write drops to ~5-10ms (estimated)
- Risk: Power loss = data loss (last uncommitted txn)
- Not recommended for production

**Option C: Reduce filter indices**
- Drop from 5 to 2-3 filter fields
- Single write: ~25ms P99 (still over budget)
- Not enough improvement

### 2. **Query Strategy: Text-first ALWAYS**

Filter-first wins ZERO scenarios in practice. Skip query planner complexity.

```rust
// Simple implementation:
pub fn search(&self, query: &str, filters: &[Filter], k: usize) -> Vec<Doc> {
    let text_results = self.bm25_search(query); // Pre-scored, sorted
    let filter_set = self.apply_filters(filters); // HashSet
    
    // Early termination
    let mut results = Vec::new();
    for doc in text_results {
        if filter_set.contains(&doc.id) {
            results.push(doc);
            if results.len() >= k { break; }
        }
    }
    results
}
```

---

## 📋 What to Do Next

### Immediate: Run Filter Selectivity Test

```bash
cargo run --release --bin filter_selectivity_test
```

This validates B-tree range query performance (the filter half of the equation).

### Then: Make Architecture Decision

**My recommendation: Batch-only writes**

Pros:
- 6,200 docs/sec per tenant (way above Algolia)
- <35ms P99 commit latency
- Simple implementation
- All 5 filter indices viable

Cons:
- 100ms write visibility delay
- Requires background commit thread
- Needs write buffering per tenant

**Alternative: Hybrid approach**
- Critical updates: NOSYNC mode (~10ms, risky)
- Bulk imports: Batched (fast)
- Most users won't notice 100ms delay for search indexing

---

## Decision Framework

| Metric | Result | Action |
|--------|--------|--------|
| Single write | ❌ 42ms P99 | Must use batching |
| Batch(100) | ✅ 0.16ms/doc | Perfect for production |
| Text-first | ✅ 0.001ms | Use always |
| Filter-first | ❌ Never wins | Skip planner |
| Early termination | ✅ 100x speedup | Use simple HashSet |

**Bottom line:** Your architecture works IF you commit batches. Single-doc real-time writes are not viable with 5 filter indices.

What's your preference: batch-only or NOSYNC mode?

----------------------------------------------------------------
# Test Results Summary - Nov 4 11pm

## Tests Completed
1. **Write Amplification Benchmark** - Document indexing with 6 B-trees (BM25 + 5 filters)
2. **Top-K Retrieval Benchmark** - Query performance with early termination

---

## Write Amplification Results

### Key Findings

**Single-document commits: FAILED**
- P99: 42.32ms (target: <15ms)
- Throughput: 38 docs/sec
- Root cause: 6 B-tree updates × 2 fsyncs per commit = 12 disk operations

**Batched commits: PASSED**
- Batch(50): 0.31ms/doc, 40.93ms P99 commit, 3,204 docs/sec
- Batch(100): 0.16ms/doc, 31.95ms P99 commit, 6,200 docs/sec ✅ **optimal**
- Batch(500): 0.045ms/doc, 26.05ms P99 commit, 22,455 docs/sec

**Interpretation:**
- fsync dominates single-doc latency (LMDB does 2 per commit)
- Batching amortizes fsync across 100 docs: 42ms → 0.16ms/doc (262x improvement)
- Batch size 100-500 hits diminishing returns

### Architectural Implications

**Real-time single-doc indexing is NOT viable with 5 filter indices.**

Options:
1. **Batch-only API** (recommended): Queue writes, commit every 100ms or 50-100 docs
   - Trade-off: 100ms write visibility delay
   - Benefit: 6,200+ docs/sec throughput per tenant
   
2. **NOSYNC mode**: Skip fsync, risk data loss on crash
   - Estimated: ~10-15ms single-doc (untested)
   - Not production-safe
   
3. **Reduce indices**: 2-3 filters instead of 5
   - Estimated: ~25-30ms single-doc (insufficient improvement)

---

## Top-K Retrieval Results

### Key Findings

**Text-first with early termination: DOMINANT**

Across all scenarios:
- P99: 0.001-0.002ms (sub-millisecond)
- Checks: Exactly 100 in high-overlap cases (perfect early stop)
- Speedup vs exhaustive: 90-117x

**Filter-first: NEVER WINS**
- P99: 0.14-0.19ms (100-130x slower than text-first)
- Only ties when filter has exactly 100 results (ultra-rare)
- Full intersection overhead negates any benefit

**Scenario Breakdown:**

| Scenario | Filter Size | Overlap | Text-first P99 | Filter-first P99 | Winner |
|----------|-------------|---------|----------------|------------------|--------|
| High selectivity | 9K | 90% | 0.002ms | 0.223ms | Text (111x) |
| Medium | 5K | 50% | 0.001ms | 0.203ms | Text (127x) |
| Low | 1K | 10% | 0.001ms | 0.138ms | Text (97x) |
| Ultra-selective | 100 | 1% | 0.191ms | 0.207ms | Tie (1.1x) |
| Boundary (filter=k) | 100 | 90% | 0.057ms | 0.216ms | Text (5x) |

### Architectural Implications

**Simple text-first strategy is optimal. No query planner needed.**

```rust
// Sufficient implementation
let mut results = Vec::new();
for doc in bm25_results { // Pre-sorted by score
    if filter_set.contains(&doc.id) {
        results.push(doc);
        if results.len() >= k { break; }
    }
}
```

**What we DON'T need:**
- ❌ Roaring bitmaps (intersection already sub-millisecond)
- ❌ WAND algorithm (early termination sufficient at 10K scale)
- ❌ Query planner (text-first wins 99.9% of cases)
- ❌ Skip lists or galloping search (HashSet lookup dominates)

---

## Critical Unknowns Remaining

1. **Filter range query performance** (test 3 pending)
   - B-tree range scan latency for `price:[500-2000]`
   - Expected: <5ms for 5K results
   
2. **Combined query latency** (not yet tested)
   - Text search (0.4ms) + filter range (estimated 2ms) + intersection (0.001ms) + sort (unknown)
   - Target: <30ms P99 combined
   
3. **Memory during batch writes** (not measured)
   - Does 100-doc batch spike RSS?
   - B-tree rebalancing overhead?

---

## Architecture Decisions Made

| Decision | Status | Rationale |
|----------|--------|-----------|
| Use batched writes (100 docs) | ✅ Required | Single-doc exceeds 15ms budget by 3x |
| Text-first with early termination | ✅ Default | 100x faster, wins all realistic cases |
| Skip query planner | ✅ Simplify | Filter-first never competitive |
| Skip Roaring bitmaps | ✅ Defer | Sub-millisecond already, premature optimization |
| 5 filter indices viable | ⚠️ If batched | Pending filter range query validation |

---

## Next Test Priority

**Filter Selectivity Test** - Validates B-tree range query performance, the missing piece for combined query latency projection.

If range queries are <5ms P99, total query budget:
- BM25: 0.4ms
- Filter range: <5ms
- Intersection: 0.001ms
- Sort (estimated): <2ms
- **Total: <8ms** (well under 30ms target)