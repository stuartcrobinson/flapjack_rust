# Nov 4 11pm Test Suite

## Purpose
Validate critical unknowns blocking architecture decisions before building the query planner.

## Tests

### 1. Write Amplification Benchmark
**File:** `write_amplification_bench.rs`

**What it tests:**
- Document write latency with 6 B-trees (1 BM25 + 5 filter indices)
- LMDB fsync overhead
- Optimal batch size for writes

**Run:**
```bash
cargo run --release --bin write_amplification_bench
```

**Success criteria:**
- Single write P99 <15ms
- Batch write (size=50) <2ms/doc average
- No memory spikes >100MB during commits

**Informs:**
- Whether filter indices blow write budget
- Required batching strategy for real-time indexing
- Memory headroom validation

**Expected results:**
- Single: 5-10ms P99 (6 B-tree updates + 2 fsyncs per commit)
- Batch(50): 0.5-2ms/doc (fsync amortized over batch)
- Sweet spot: batch_size=50-100

**If fails:**
- P99 >15ms → Need LMDB NOSYNC mode or reduce filter indices
- Batch still slow → Consider LSM-tree instead of B-tree
- Memory spike → Filter indices too expensive per tenant

---

### 2. Top-K Retrieval Benchmark
**File:** `topk_retrieval_bench.rs`

**What it tests:**
- Early termination effectiveness vs exhaustive intersection
- Text-first vs filter-first performance
- Actual checks needed to find top-100

**Run:**
```bash
cargo run --release --bin topk_retrieval_bench
```

**Success criteria:**
- Text-first P99 <5ms
- Early termination stops at <1000 checks
- Text-first wins in most scenarios

**Informs:**
- Whether exhaustive intersection is needed (NO if early termination <5ms)
- When to use filter-first
- If Roaring bitmaps or WAND are needed

**Expected results:**
- Text-first: <1ms P99, stops at ~200-500 checks
- Filter-first: Only wins when filter <100 results
- Exhaustive: 5-10x slower (validates early termination value)

**If fails:**
- P99 >5ms → Need better intersection algorithm
- Early termination checks >5K → May need WAND pruning
- Filter-first never wins → Skip query planner

---

### 3. Filter Selectivity Test
**File:** `filter_selectivity_test.rs`

**What it tests:**
- Crossover point where filter-first beats text-first
- B-tree range cardinality estimation accuracy
- Query planner decision threshold

**Run:**
```bash
cargo run --release --bin filter_selectivity_test
```

**Success criteria:**
- Clear crossover point detected
- Simple threshold (e.g., <1000 results) sufficient
- Both strategies <10ms P99

**Informs:**
- Query planner heuristic design
- Whether cost-based optimization is needed
- Filter index value proposition

**Expected results:**
- Crossover at ~5-10% selectivity (500-1000 filter results)
- Simple heuristic: `if filter_cardinality < 1000 { filter_first() }`
- Text-first default is correct

**If fails:**
- No clear crossover → Skip query planner complexity
- Crossover >50% → Query planner critical
- Both strategies slow → Fundamental perf problem

---

## Decision Matrix

### After Test 1 (Write Amplification)
| Result | Decision |
|--------|----------|
| P99 <10ms, batch <1ms/doc | ✅ Proceed with 5 filter indices |
| P99 10-15ms | ⚠️ Reduce to 3 filter indices or enable NOSYNC |
| P99 >15ms | ❌ Reconsider B-tree, explore LSM-tree |

### After Test 2 (Top-K Retrieval)
| Result | Decision |
|--------|----------|
| P99 <2ms, checks <500 | ✅ Use simple early termination |
| P99 2-5ms, checks <2K | ⚠️ Optimize but sufficient |
| P99 >5ms or checks >5K | ❌ Need WAND or Roaring bitmaps |

### After Test 3 (Filter Selectivity)
| Result | Decision |
|--------|----------|
| Crossover <10% selectivity | ✅ Simple threshold heuristic |
| Crossover 10-30% | ⚠️ Consider cost-based planner |
| No crossover or >30% | ❌ Always text-first, skip planner |

---

## What to Build Next

**If all tests pass:**
1. Implement simple query planner with threshold heuristic
2. Build filter range query API
3. Test combined text+filter+sort P99 <30ms
4. Move to write path validation

**If Test 1 fails:**
- Research: LMDB NOSYNC mode safety
- Benchmark: 3 filter indices vs 5
- Consider: WAL-based approach

**If Test 2 fails:**
- Research: WAND algorithm implementation
- Consider: Roaring bitmaps for intersection
- Benchmark: Skip list vs galloping search

**If Test 3 fails:**
- Simplify: Always use text-first
- Skip: Query planner complexity
- Focus: Filter API for simple AND queries

---

## Key Insights from Research

1. **LMDB Write Performance** ([source](https://en.wikipedia.org/wiki/Lightning_Memory-Mapped_Database))
   - 2 fsyncs per commit (data + meta page)
   - Batching critical for throughput
   - NOSYNC mode available but risks data loss

2. **B-tree Write Amplification** ([sources](https://arxiv.org/pdf/2107.13987))
   - Single insert can trigger page splits up to root
   - ~2-4x write amplification typical
   - Multiple indices multiply this cost

3. **Top-K Query Patterns** ([sources](https://docs.vespa.ai/en/using-wand-with-vespa.html))
   - Production systems use WAND for >100M docs
   - Early termination sufficient for <100K docs
   - Text-first default is correct for most queries

4. **Filter Selectivity** ([sources](https://www.alibabacloud.com/blog/analyzing-elasticsearch-performance-with-lucene_594636))
   - Lucene switches between scan and filter-first based on cardinality
   - Crossover typically at ~5-10% selectivity
   - B-tree range bounds check enables fast estimation

---

## Critical Questions These Tests Answer

1. ✅ Does 5 filter indices blow write budget? (Test 1)
2. ✅ Is exhaustive intersection needed? (Test 2)
3. ✅ When should we use filter-first? (Test 3)
4. ✅ Is early termination sufficient? (Test 2)
5. ✅ Do we need Roaring bitmaps? (Test 2)
6. ✅ Is query planner worth the complexity? (Test 3)

---

## Next Round of Tests (If Current Tests Pass)

1. **Combined Query Test:** text + filter + sort in single query
2. **Multi-Tenant Write Load:** 20 tenants × 100 writes/sec
3. **Memory Pressure Test:** RSS during concurrent read/write
4. **Facet Aggregation:** Count docs per filter bucket
5. **Migration Test:** Copy index + filters in <100ms

These tests validate the **complete query path** and **production load handling**.

https://claude.ai/chat/46629a02-dc90-4489-9e75-96b8dde01623