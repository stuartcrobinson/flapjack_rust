
removed from /Users/stuart/repos/flapjack_rust/src/bin/nov6_1040am/BUILD_PLAN.md

**One critical gap:** Query planner threshold (1200 docs) came from LMDB tests with in-memory data structures. **Validate this holds with Tantivy B-tree scans** by running your filter selectivity test from `nov4_1100pm/FINAL_RESULTS.md` but on actual Tantivy indices, not LMDB. Takes 2 hours. If crossover point shifts significantly (e.g., 5000 docs), query planner logic changes.

Otherwise: build, measure, iterate. Testing without production context has diminishing returns.


----------------------


**Critical finding: Crossover at ~1200 docs validated.**

Filter-first wins at 890 docs (8.9%), loses at 1853 docs (18.5%). Your LMDB threshold holds.

---

## Anomaly: Text-first speeds up as filter cardinality increases

**Expected:** Text-first scans BM25 results sequentially, stops at k=100 matches. Higher filter cardinality = more docs pass filter = fewer BM25 results scanned = faster.

**Observed:**
- 890 filtered docs → 4.43ms
- 1853 filtered docs → 2.11ms  
- 4944 filtered docs → 0.22ms (20x faster!)
- 7970 filtered docs → 0.17ms

**Hypothesis 1: Early termination kicking in**
At 79.7% selectivity, almost every BM25 result passes filter. Text-first collects k=100 in first ~125 results, exits immediately. At 8.9% selectivity, must scan ~1000 BM25 results to find 100 matches.

**Validates design assumption:** Text-first wins when filter is non-selective (most docs pass). Filter-first wins when highly selective (few docs to score).

---

## Phase 1.3 query planner threshold confirmed

```
if filter_cardinality < 1200:
    use filter-first
else:
    use text-first
```

Ship it. No adjustment needed.

---

## Remaining test gaps

**1. Does this hold at 100K+ docs?**  
Test showed 10K corpus. If segment structure changes performance characteristics at scale, crossover might shift. Quick validation: rerun with `n_docs = 100_000`, see if threshold stays ~1200 or moves to ~12K (10x scale).

**2. Multiple segments impact**  
Your test commits once → single segment. Production: continuous writes → multiple segments. Does filter-first segment iteration overhead change crossover? Test by committing every 1K docs during index creation.

**3. Filter on multiple fields**  
Test used single field (price). Real queries: `price:[100-1000] AND category:"electronics"`. Does combined filter selectivity follow same threshold? Likely yes (cardinality math is additive), but validate if time permits.

**4. Non-uniform BM25 distribution**  
"laptop" matches 50% of docs uniformly. Real corpus: Zipf distribution (few docs highly relevant, long tail). If top-100 BM25 results cluster at top-1000, text-first early termination is even faster. Current test may underestimate text-first advantage.

---

## Ship or test more?

**Ship.** Threshold validated on realistic workload. Remaining gaps are second-order effects. Defer to production telemetry—if P99 query latency regresses after launch, profile and adjust threshold.

**One quick win:** Add 100K doc test (10 min runtime). If crossover still ~1200, you have scale confidence. If it shifts to 5K+, you know threshold needs to be `min(1200, corpus_size * 0.12)` or similar.