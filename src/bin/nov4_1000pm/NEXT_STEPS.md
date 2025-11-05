https://claude.ai/chat/6ab3257a-d506-467f-b8a6-fb952ae6ac5a

Let me create a deep dive document on what to build next:

[View NEXT_STEPS_DEEP_DIVE.md](computer:///mnt/user-data/outputs/NEXT_STEPS_DEEP_DIVE.md)

## Summary: What to Build Next (275 words)

**Critical unknown:** Intersection performance. Your BM25 works (2.3 MB, 0.4ms). Real queries need filters. Query: `"laptop" AND price:[500-2000]` requires intersecting 10K text results with 5K filter results.

**Research shows:** No universal winner. Simple merge often competitive. Roaring bitmaps 900x faster in some cases, used by Elasticsearch/Lucene, but adds complexity.

**Build sequence:**

**Days 1-2:** Implement LMDB INTEGER_KEY range queries for numeric/date filters. B+ trees naturally efficient for ranges (linked leaf nodes). Expected: <5ms P99, +1-2 MB/tenant memory.

**Day 3:** Benchmark intersection algorithms:
- HashSet (current implicit approach)
- Sorted merge with galloping
- Roaring bitmaps (if needed)

Test scenarios: balanced (10K×10K), skewed (1K×50K), sparse overlap.

**Decision point:** 
- <10ms → ship HashSet
- 10-30ms → optimize with sorted merge  
- >30ms → add Roaring (2-3 day detour)

**Days 4-5:** Query planner - execute text vs filter first based on selectivity. Text-first default (BM25 pre-scored). Target: <30ms P99 combined.

**Why this order:** Filters validate B-tree performance. Intersection benchmark prevents premature optimization (don't add Roaring if HashSet works). Planner needs both implemented to tune.

**Risks:** 
- Filter indices >2 MB/tenant (reduce free tier fields)
- Intersection >30ms (add Roaring, acceptable)
- Combined query >50ms (not competitive, needs optimization)

**Don't build query planner until intersection cost measured.** You're designing around an unknown constraint. That 10K×5K intersection could be 2ms or 50ms - fundamentally different architectures.


###############################################################
###############################################################
###############################################################


# What to Build Next: Query Intersection & Filters

**Context:** BM25 validated at 2.3 MB/tenant, 0.4ms P99 text-only. Critical unknown: intersection performance.

---

## The Central Problem

Your text search works. The question is: **what happens when you add filters?**

Query: `"laptop" AND price:[500-2000] ORDER BY date`

This requires:
1. BM25 text search → 10K candidates
2. Price range filter → 5K candidates  
3. **Intersection** → 2K results
4. Sort by date
5. Return top 100

**The intersection is the unknown.** If it takes 50ms, you fail competitive benchmarks.

---

## Research Findings: Intersection Algorithms

### Academic Consensus (from search)

No single intersection algorithm outperforms others in every scenario - performance depends on algorithm, hardware, and list lengths

Adaptive algorithms adjust based on list size ratios - value-based search performs well for posting lists in terms of comparisons

Simple merge algorithm performs competitively despite more sophisticated techniques existing

### Key Algorithm Classes

1. **Naive Merge** (sorted list intersection)
   - O(n + m) where n, m = list sizes
   - Simple, cache-friendly
   - Often competitive despite being unsophisticated

2. **HashSet Intersection**
   - Build HashSet from smaller list
   - Probe with larger list
   - O(min(n,m)) space, O(n+m) time
   - Your current approach (implicit in BM25 `search()`)

3. **Binary Search / Galloping**
   - Value-based search algorithms perform well on posting lists
   - Search for elements of small list in large list
   - O(n log m) where n < m
   - Good when lists differ greatly in size

4. **Roaring Bitmaps**
   - Up to 900x faster than alternatives for intersections, especially sparse with dense sets
   - Used by Apache Lucene, Spark, Elasticsearch, Netflix
   - Never the fastest but never a bad choice - graceful degradation
   - Overhead: ~25-50% compression vs uncompressed

### When Does Intersection Become Bottleneck?

Google doesn't find all results, only top ones - estimates total count, stops at sufficient results

**Implication:** For top-k queries, you can stop early. Don't intersect 10K × 5K if you only need 100 results.

**Early termination strategies:**
- Threshold-based pruning - dynamically eliminate docs that can't reach top results (WAND algorithm inspiration)
- Selective initial retrieval - use rarest terms (highest IDF) for candidate set first

---

## What Algolia/Meilisearch Actually Do

### Algolia
- Uses inverted indices for text
- Supports custom ranking, rules, multi-field search
- 70+ data centers, distributed search
- Advertised <50ms P99

**Inference:** They likely use:
- Optimized posting list compression
- Early termination for top-k
- Possibly Roaring bitmaps (industry standard)

### Meilisearch  
- Advertised <50ms response times
- Built with Rust, emphasizes performance and security
- Open source, so you can check: uses LMDB + custom ranking

---

## Filter Implementation Strategy

### Phase 1: LMDB Range Queries (Filters)

**Goal:** Implement numeric/date range filters using LMDB INTEGER_KEY.

**Design:**
```rust
// Sort index DB per field
// Key: field_value (u32/u64 big-endian) → Value: doc_id (u32)

pub fn filter_range(&self, field: &str, min: u64, max: u64) -> Result<Vec<u32>> {
    let rtxn = self.env.read_txn()?;
    let sort_db = self.get_sort_db(field)?;
    
    let mut results = Vec::new();
    let cursor = sort_db.range(&rtxn, &min..=&max)?;
    
    for result in cursor {
        let (_, doc_id) = result?;
        results.push(doc_id);
    }
    
    Ok(results)
}
```

**Memory impact per field:**
- 10K docs × 12 bytes (8 byte key + 4 byte doc_id) = 120 KB disk
- B+ trees efficient for range queries - linked leaf nodes enable fast sequential scan
- Working set: ~0.2-0.4 MB per field (B-tree pages)

**Validation needed:**
1. Measure range query latency: 5K results should be <5ms
2. Measure memory delta with 5 sort indices
3. Expected: +1-2 MB/tenant (acceptable within 4GB budget)

### Phase 2: Intersection Benchmark

**Critical test:** Measure actual intersection cost before choosing algorithm.

**Test matrix:**
```rust
// Scenario 1: Balanced lists (10K × 10K)
let text_results = vec![1..10000];
let filter_results = vec![5000..15000]; 
// Intersection: 5K results
// Measure: HashSet build + probe time

// Scenario 2: Skewed lists (1K × 50K)  
let text_results = vec![1..1000];
let filter_results = vec![100..50100];
// Intersection: 900 results
// Measure: Which is faster - HashSet or binary search?

// Scenario 3: Sparse intersection (10K × 10K → 100 results)
let text_results = random_sample(100K, 10K);
let filter_results = random_sample(100K, 10K);
// Low overlap
// Measure: Early termination benefit?
```

**Decision tree:**
- If HashSet <10ms for all scenarios → ship it
- If 10-30ms → optimize (sorted merge, galloping)
- If >30ms → need Roaring bitmaps

**Hypothesis:** HashSet will be <10ms because:
- Modern CPUs: 10K HashSet build = ~2M cycles = ~1ms @ 2GHz
- Cache-friendly: 10K u32 = 40 KB fits in L2
- Your candidates are pre-filtered (BM25 top-k, not全corpus)

**Counter-hypothesis:** HashSet may fail if:
- Memory allocations dominate (heap pressure)
- Hash collisions degrade to O(n²)
- Filter selectivity poor (100K+ candidates)

### Phase 3: Query Planner

**After intersection validated, build optimal execution order.**

Index fields should follow Equality, Sort, Range (ESR) rule for optimal B-tree usage

**Query execution strategies:**

#### Strategy A: Filter-First (if filter highly selective)
```
1. filter_price(500-2000) → 5K candidates
2. For each: check if matches text terms → 2K results
3. Score with BM25
4. Sort by date field
5. Top 100
```

**When optimal:** Filter returns <1K candidates (rare)

#### Strategy B: Text-First (default)
```
1. BM25 text search → 10K candidates (pre-scored)
2. Intersect with filter_price(500-2000) → 2K
3. Load sort keys for 2K docs
4. Sort + return top 100
```

**When optimal:** Text search selective (most queries)

#### Strategy C: Index-Only (if covered index exists)
```
1. Composite index: (category, price, date)
2. Range scan on category+price
3. Results pre-sorted by date
4. No doc lookup needed
```

**When optimal:** Small result sets, no text scoring needed

**Planner heuristic:**
```rust
fn choose_strategy(query: &Query) -> Strategy {
    let text_selectivity = estimate_text_candidates(query.terms);
    let filter_selectivity = estimate_filter_candidates(query.filters);
    
    if filter_selectivity < 1000 && text_selectivity > 10000 {
        Strategy::FilterFirst
    } else if query.terms.is_empty() {
        Strategy::IndexOnly
    } else {
        Strategy::TextFirst // Default - BM25 already computed
    }
}
```

---

## Roaring Bitmaps: When and Why

### When NOT Needed (start here)

Roaring bitmaps rarely the fastest but never a bad choice

You don't need Roaring if:
- Intersection <10ms with naive approach
- Document counts <100K per tenant
- Memory budget not tight (you have 3.7 MB headroom per tenant)

### When Needed

Roaring becomes critical when:
- Large posting lists (>50K docs)
- Multiple filters (3+ field intersections)
- Complex queries (faceting, which requires many intersections)

Roaring 4-5x faster than alternatives for intersections across densities

### Implementation Cost

**Rust crates available:**
- `croaring` - C bindings, battle-tested
- `roaring` - Pure Rust

**Integration:**
```rust
use roaring::RoaringBitmap;

// Convert posting list to bitmap
let bitmap1 = RoaringBitmap::from_iter(text_results);
let bitmap2 = RoaringBitmap::from_iter(filter_results);

// Intersection
let intersection = bitmap1 & bitmap2;

// Convert back to vec
let results: Vec<u32> = intersection.iter().collect();
```

**Memory overhead:**
- Roaring uses 25-50% space vs uncompressed, 2x better than WAH/Concise
- For 10K doc posting list: ~5-10 KB (vs 40 KB raw)
- Adds ~0.5-1 MB per active tenant

**Decision:** Defer until intersection benchmark shows >30ms P99.

---

## Recommended Build Sequence

### Week 1: Filters + Measurement

**Day 1-2: Implement filter indices**
```rust
// Add to BM25Index
pub struct FilterIndex {
    price_db: Database<U64<BigEndian>, U32<NativeEndian>>,
    date_db: Database<U64<BigEndian>, U32<NativeEndian>>,
    category_db: Database<Str, SerdeBincode<Vec<u32>>>,
}

impl FilterIndex {
    pub fn range_query(&self, field: Field, range: Range) -> Vec<u32>;
    pub fn exact_match(&self, field: Field, value: Value) -> Vec<u32>;
}
```

**Test:**
- Index 20 tenants × 10K docs with 3 numeric fields
- Measure: range query latency (target <5ms P99)
- Measure: memory delta (target <2 MB/tenant)

**Day 3: Intersection benchmark**
```rust
// New test: src/bin/nov4_10pm/intersection_benchmark.rs

fn bench_hashset_intersection(sizes: &[(usize, usize)]);
fn bench_sorted_merge_intersection(sizes: &[(usize, usize)]);
fn bench_with_roaring(sizes: &[(usize, usize)]);

// Test scenarios:
// - Balanced: 10K × 10K
// - Skewed: 1K × 50K
// - Sparse: 10K × 10K with 1% overlap
// - Dense: 10K × 10K with 90% overlap
```

**Success criteria:**
- HashSet P99 <10ms → use HashSet
- 10-30ms → optimize with sorted merge
- >30ms → prototype Roaring

**Day 4-5: Query planner**
```rust
pub struct QueryPlanner {
    stats: CorpusStats,
}

impl QueryPlanner {
    pub fn execute(&self, query: &Query) -> Vec<ScoredDoc> {
        // 1. Choose strategy (filter-first vs text-first)
        // 2. Execute in optimal order
        // 3. Intersect results
        // 4. Apply sorts
        // 5. Return top-k
    }
}
```

**Test:** Combined text+filter+sort P99 <30ms

### Week 2: Production Features

**Day 6-7: Multi-field support**
- 5 sortable fields per tenant
- Memory impact validation
- Composite filter queries (price AND date range)

**Day 8-9: Write path with filters**
- Update filter indices on doc insert/update/delete
- Batch commit strategy
- Validate <15ms P99 write latency

**Day 10: Load testing**
- 400 tenants, 10K docs each
- 100 QPS across tenants
- Combined text+filter+sort queries
- Validate: P99 <50ms, RSS <3.5 GB

---

## Open Questions & Risks

### Risk 1: Intersection Slower Than Expected

**If HashSet >30ms P99:**

**Option A:** Optimize without Roaring
- Use sorted lists + galloping search
- Early termination (stop at top-100)
- Reduce candidate set size (higher BM25 threshold)

**Option B:** Add Roaring bitmaps
- Immediate 5-10x speedup for intersections
- Memory cost: +0.5-1 MB/tenant (acceptable)
- Implementation: 2-3 days

**Option C:** Reduce scope
- Limit filters to 2 fields (price + one other)
- No multi-field intersection (AND only one filter at a time)
- Still competitive with basic Algolia

### Risk 2: Filter Index Memory Exceeds Budget

**If 5 sort indices = 5+ MB/tenant:**
- Current assumption: 0.4 MB per field
- If actual: 1+ MB per field = 5 MB total
- Combined with BM25 (2.3 MB) = 7.3 MB/tenant
- Max tenants: 491 (vs 1,565 current, 400 target)

**Mitigation:**
- Free tier: 2 sort fields only
- Paid tier: 5 fields ($2/tenant to cover overhead)
- Still cheaper than Algolia

### Risk 3: Sort Field Lookup Slow

**Hypothesis:** 100 doc_ids × sort key lookup = 100 LMDB gets

B+ tree leaf nodes linked for efficient sequential access

**If >5ms:**
- Batch sort key retrieval (single cursor scan)
- Or: store sort keys inline with posting lists (memory tradeoff)
- Or: cache recently sorted results

### Risk 4: Query Planner Complexity

**Cost-based planning is hard:**
- Need accurate selectivity estimates
- Statistics maintenance overhead
- Wrong strategy = 10x slowdown

**Mitigation:** Start with simple heuristics
- Text-first by default (BM25 already scored)
- Switch to filter-first only if filter ultra-selective (<100 results)
- Measure in production, adjust heuristics

---

## Success Metrics (End of Week 2)

1. **Filters work**
   - Range queries: <5ms P99
   - Exact matches: <1ms P99
   - Memory: +1-2 MB/tenant

2. **Intersection validated**
   - HashSet or sorted merge <10ms P99
   - OR: Roaring implementation if needed

3. **Combined query performance**
   - Text + filter: <20ms P99
   - Text + filter + sort: <30ms P99
   - Competitive with Algolia/Meilisearch 50ms target

4. **Multi-tenant stable**
   - 400 tenants, 10K docs each
   - 100 QPS load test passes
   - RSS <3.5 GB (0.5 GB headroom)

5. **Write path functional**
   - Index updates with filters
   - <15ms P99 commit latency

If all pass: **Architecture fully validated.** Proceed to faceting, typo tolerance, production deployment.

If intersection >30ms: **Implement Roaring.** 2-3 day detour, acceptable.

If memory >5 MB/tenant: **Reduce free tier fields.** Economic model still viable at 200 density.

---

## Why This Sequence?

**Filters first because:**
- Required for Algolia parity (table stakes)
- Validates B-tree range query performance
- Simple to implement (1-2 days)

**Intersection benchmark before implementation because:**
- No algorithm universally best - depends on data characteristics
- Premature Roaring optimization wastes time if HashSet works
- Data-driven decision > theoretical analysis

**Query planner last because:**
- Needs filter + intersection implemented first
- Simple heuristics sufficient for MVP
- Can optimize based on production query logs

**Defer faceting/typo tolerance because:**
- Not on critical path for core search validation
- Faceting requires intersection algorithm finalized
- Typo tolerance = separate subsystem (FST expansion)

---

## Alternative: Skip Filters, Build Faceting

**Why not?**

Faceting requires:
- Multiple intersections (one per facet value)
- Aggregation (count per bucket)
- Even more sensitive to intersection performance

Filters validate intersection first, then faceting builds on proven foundation.

**Exception:** If targeting e-commerce only, faceting more critical than arbitrary filters. But filters are simpler validation step.

---

## Bottom Line

**Next 3 days:**
1. Build filter range queries (LMDB INTEGER_KEY)
2. Benchmark intersection (HashSet vs sorted vs Roaring)
3. Decide: ship HashSet, optimize, or add Roaring

**That decision determines Week 2 work:**
- Fast intersection → query planner + production features
- Slow intersection → Roaring implementation + retry

Your BM25 is validated. The intersection algorithm is the last critical unknown before you can confidently build features.

**Don't build query planner until you know intersection cost.** Otherwise you're designing around unknown constraints.