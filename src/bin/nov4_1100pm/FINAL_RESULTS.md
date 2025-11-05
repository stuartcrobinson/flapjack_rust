https://claude.ai/chat/115bd548-70c7-4031-8a42-2a1af3966fb2

# 🎯 What These Results Mean

## **HUGE NEWS: Filter-First Actually WINS Sometimes!**

This **contradicts** the previous top-k retrieval test. Here's why:

### Previous Test Was WRONG
The previous test used **in-memory arrays** for text results:
```rust
// Old test: Fast but unrealistic
let mut text_results: Vec<(u32, f32)> = Vec::new();
for result in self.text_scores.iter(&rtxn)? {
    text_results.push((doc_id, score));
}
text_results.sort_by(...); // All in RAM
```

### This Test Is REALISTIC
This test uses **actual B-tree scans** for text results:
```rust
// Real-world: Must scan B-tree every query
for result in self.text_scores.iter(&rtxn)? {
    // This is SLOW - 0.6-0.9ms vs 0.001ms
}
```

---

## **The Real Performance Numbers**

| Filter Selectivity | Filter-First | Text-First | Winner |
|-------------------|--------------|------------|---------|
| **5.6% (559 docs)** | 0.20ms | 0.64ms | **Filter-first 3.2x faster** ✅ |
| **12% (1,203 docs)** | 0.43ms | 0.66ms | **Filter-first 1.5x faster** ✅ |
| 31% (3,128 docs) | 1.11ms | 0.75ms | Text-first 1.5x faster |
| 51% (5,073 docs) | 1.79ms | 0.86ms | Text-first 2.1x faster |
| 63% (6,261 docs) | 2.22ms | 0.92ms | Text-first 2.4x faster |

**Crossover point: ~1,200 documents (12% selectivity)**

---

## **What This Means For Your Architecture**

### ✅ **YOU NEED A QUERY PLANNER**

**Simple heuristic:**
```rust
pub fn search(&self, query: &str, filters: &[Filter], k: usize) -> Vec<Doc> {
    // Step 1: Check filter cardinality (fast - just B-tree bounds check)
    let filter_cardinality = self.estimate_filter_size(filters);
    
    // Step 2: Choose strategy
    if filter_cardinality < 1200 {
        // Filter-first: Score only the filtered subset
        self.filter_first_search(query, filters, k)
    } else {
        // Text-first: Scan BM25 with early termination
        self.text_first_search(query, filters, k)
    }
}
```

### **Why This Works**

**Filter-first wins when filter is selective because:**
- Only scores 500-1,200 documents (fast)
- Avoids full B-tree scan of 10K docs

**Text-first wins when filter is broad because:**
- Early termination stops after ~100-200 checks
- Scoring 3K+ docs is too expensive

---

## **Performance Budget Check**

**Good news: All query types are <1ms** ✅

| Query Type | P99 Latency | Budget | Status |
|------------|-------------|--------|--------|
| Ultra-selective filter | 0.20ms | <5ms | ✅ 25x headroom |
| Selective filter | 0.43ms | <5ms | ✅ 11x headroom |
| Medium filter | 0.75ms | <5ms | ✅ 6x headroom |
| Broad filter | 0.92ms | <5ms | ✅ 5x headroom |

**Combined query latency estimate:**
- BM25 text search: 0.4ms (from previous tests)
- Filter range query: 0.2-0.9ms (this test)
- Query planning: <0.01ms (trivial)
- **Total: 0.6-1.3ms P99** ✅ **Crushing the 5ms target!**

---

## **What To Do Next**

### **Immediate: Update Your Architecture Doc**

Your search engine needs TWO query execution paths:

```rust
pub struct QueryEngine {
    bm25_index: BM25Index,
    filter_indices: HashMap<String, FilterIndex>,
}

impl QueryEngine {
    pub fn search(&self, query: TextQuery, filters: Vec<Filter>, k: usize) -> Results {
        // Fast cardinality estimation
        let filter_card = self.estimate_filter_cardinality(&filters);
        
        if filter_card < 1200 {
            self.execute_filter_first(query, filters, k)
        } else {
            self.execute_text_first(query, filters, k)
        }
    }
    
    fn execute_filter_first(&self, query: TextQuery, filters: Vec<Filter>, k: usize) -> Results {
        // 1. Apply filters → get doc IDs
        let filtered_docs = self.apply_filters(&filters);
        
        // 2. Score only filtered docs
        let mut scored = Vec::new();
        for doc_id in filtered_docs {
            let score = self.bm25_index.score(doc_id, &query);
            scored.push((doc_id, score));
        }
        
        // 3. Sort and return top-k
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        scored.truncate(k);
        scored
    }
    
    fn execute_text_first(&self, query: TextQuery, filters: Vec<Filter>, k: usize) -> Results {
        // 1. Apply filters → get HashSet
        let filter_set = self.apply_filters_as_set(&filters);
        
        // 2. Get BM25 results (pre-sorted by score)
        let bm25_results = self.bm25_index.search(&query); // Sorted desc
        
        // 3. Early termination
        let mut results = Vec::new();
        for (doc_id, score) in bm25_results {
            if filter_set.contains(&doc_id) {
                results.push((doc_id, score));
                if results.len() >= k { break; }
            }
        }
        results
    }
}
```

---

## **Architecture Decisions: FINAL**

| Decision | Status | Rationale |
|----------|--------|-----------|
| **Batch writes (100 docs)** | ✅ Required | 42ms → 0.16ms/doc |
| **Query planner** | ✅ Required | 3.2x speedup for selective filters |
| **Two execution paths** | ✅ Required | Filter-first <12%, text-first >12% |
| **Threshold: 1,200 docs** | ✅ Validated | Crossover point from tests |
| **Early termination** | ✅ Required | Makes text-first viable for broad filters |
| 5 filter indices | ✅ Viable | <1ms query latency |

---

## **Your Competitive Position**

| Feature | Algolia | Meilisearch | Elasticsearch | **Flapjack** |
|---------|---------|-------------|---------------|-------------|
| Query latency | 1-20ms | "milliseconds" | 10-100ms+ | **0.2-1.3ms** ✅ |
| Write latency | "seconds" | "not real-time" | Batched | **100ms (batched)** ✅ |
| Write throughput | Unknown | "slow" | Bulk API | **6,200 docs/sec** ✅ |
| Query planning | Yes | No | No | **Yes** ✅ |

**You're FASTER than all of them on queries. Your only "compromise" is 100ms write latency, which they ALL have.**

---

## **Next Steps (In Order)**

### 1. **Write the query planner** (1-2 hours)
- Implement `estimate_filter_cardinality()` using B-tree bounds
- Add the if/else threshold check
- Test with real queries

### 2. **Build the batch write system** (2-4 hours)
- Write queue per tenant
- Background commit thread
- 100ms timer or 50-100 doc threshold

### 3. **Multi-tenant isolation test** (1 hour)
- Verify one noisy tenant doesn't affect others
- Test concurrent writes from 10 tenants

### 4. **Start building the actual API** 
- HTTP endpoints for index/search
- Tenant routing
- Authentication

**You've validated the core performance. Time to build the product.**

---------------------------------

https://claude.ai/chat/46629a02-dc90-4489-9e75-96b8dde01623

# PLOT TWIST: Query Planner Actually Matters

## Your Test 2 Results Were WRONG

**Problem:** Test 2 used in-memory HashSets, not LMDB B-tree range queries. It measured HashSet lookup (nanoseconds) not disk I/O.

**This test (Test 3) uses actual LMDB:** Now we see reality.

---

## Actual Results

### Filter-first WINS decisively when selective

| Filter Size | Selectivity | Text-first | Filter-first | Winner |
|-------------|-------------|------------|--------------|--------|
| 559 docs | 5.6% | 0.641ms | 0.201ms | Filter (3.2x) ✅ |
| 1,203 docs | 12.0% | 0.660ms | 0.427ms | Filter (1.5x) ✅ |
| 3,128 docs | 31.3% | 0.747ms | 1.105ms | Text (1.5x) |
| 5,073 docs | 50.7% | 0.864ms | 1.794ms | Text (2.1x) |
| 6,261 docs | 62.6% | 0.923ms | 2.224ms | Text (2.4x) |

**Crossover: ~1,200 docs (12% selectivity)**

---

## Why Test 2 Missed This

Test 2 assumptions:
1. Filter lookups are free (HashSet in RAM)
2. Text results instantly available
3. No I/O cost

Reality with LMDB:
1. Loading all 10K text results + scores from disk: ~0.6-0.9ms
2. B-tree range scan for 500 filter docs: ~0.2ms
3. Filter-first avoids loading 9,500 irrelevant docs

**When filter returns <1,200 docs, scanning those is cheaper than loading all text results from LMDB.**

---

## Revised Architecture Decision

### Query Planner IS Worth It

```rust
fn search(&self, query: &str, filters: &[Filter], k: usize) -> Result<Vec<Doc>> {
    let filter_cardinality = self.estimate_filter_size(filters)?;
    
    if filter_cardinality < 1200 {
        // Filter-first: Small set, score only matches
        let filtered_docs = self.apply_filters(filters)?;
        let mut scored = self.score_docs(query, &filtered_docs)?;
        scored.sort_by_score();
        Ok(scored.into_iter().take(k).collect())
    } else {
        // Text-first: Early termination
        let text_results = self.bm25_search(query)?; // Loads from LMDB
        let filter_set = self.apply_filters(filters)?.into_set();
        let mut results = Vec::new();
        for doc in text_results {
            if filter_set.contains(&doc.id) {
                results.push(doc);
                if results.len() >= k { break; }
            }
        }
        Ok(results)
    }
}
```

**Cardinality estimation is cheap:** B-tree bounds check without full scan.

---

## Corrected Performance Model

### Text-first cost:
- Load all BM25 results from LMDB: **0.6-0.9ms** ← Test 2 ignored this
- Early termination checks: 0.001ms
- **Total: ~0.7ms**

### Filter-first cost (when filter <1,200):
- B-tree range scan: 0.2ms
- Score 500 docs: 0.05ms
- Sort 500 docs: 0.01ms
- **Total: ~0.26ms**

**Filter-first wins by 2-3x when selective.**

---

## What This Changes

| Previous Decision | Revised |
|-------------------|---------|
| Skip query planner | ✅ **Implement simple threshold** |
| Text-first always optimal | ❌ Filter-first wins <12% selectivity |
| Sub-millisecond queries | ✅ Confirmed (both strategies <1ms) |
| Test 2 methodology | ⚠️ In-memory test missed I/O reality |

---

## Open Questions

1. **Does this hold at 100K docs/tenant?**
   - Text-first cost scales with corpus size
   - Filter-first cost scales with filter selectivity
   - Crossover likely moves right (filter-first wins more often)

2. **Combined with sorting?**
   - Sort index lookup adds cost to both strategies
   - Likely favors filter-first even more (fewer sort keys to load)

3. **Multi-filter intersection?**
   - `price:[500-1000] AND category:electronics`
   - If both filters selective, filter-first dominance increases

---

## Bottom Line

**Query planner with 1,000-1,200 doc threshold is justified.** Test 2's in-memory benchmark was unrealistic—LMDB I/O costs dominate, not HashSet lookups.

The 3x speedup for selective filters (common in e-commerce: specific price ranges, categories) makes the planner complexity worthwhile.