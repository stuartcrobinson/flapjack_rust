https://claude.ai/chat/3ccc21c7-1b49-4e83-9a04-615d35d443c3

# Tantivy Memory Architecture: Consolidated Analysis

## Critical Finding

**Per-tenant operational cost is 0.776 MB (hot) / 0.4 MB (cold), not 2.38 MB.**

The original `realistic_density_test` measurement was wrong. It measured total RSS including malloc fragmentation from 150 sequential IndexWriter allocations (150 × 50MB buffers = 7.5GB allocated), not isolated query working set.

## Measurement Error Breakdown

**Flawed test baseline:** 317 MB after index creation  
**Clean baseline:** 136 MB (181 MB difference = malloc fragmentation)

The test calculated `(360 MB final - 3 MB initial) / 150 = 2.38 MB` but this included:
- Index structures faulted during creation (not query working set)
- Arena allocator fragmentation never returned to OS
- Reader metadata for all 150 tenants simultaneously
- Page cache contamination from sequential creation

**Correct methodology** (from `query_working_set_test_v2`): Measure RSS delta after clean baseline, isolating query working set from creation artifacts.

## Validated Cost Model

**Cold tenant:** 0.4 MB (reader metadata only)  
**Hot tenant:** 1.2 MB (0.4 MB reader + 0.776 MB working set)

Tested with realistic schema (5 FAST fields, 50K docs/tenant):
- 150 tenants all-hot: 175 MB observed (1.17 MB/tenant) ✓
- 120-tenant LRU: 90 MB observed (0.75 MB/hot tenant) ✓

## Index::open() Cost

Opening Index objects consumes **0.00 MB RSS**. Memory-mapped files don't consume RSS until pages are accessed via queries. This invalidates the premise that opening many indexes creates memory pressure.

## Capacity Analysis

**Conservative (all tenants hot):**  
1330 MB / 1.2 MB = 1108 tenants max  
60% safety margin: **665 tenants**

**Realistic (Zipf: 20% hot, 80% cold):**  
Blended: 0.56 MB/tenant → 2375 tenants max  
60% safety margin: **1425 tenants**

**Your 600-tenant target:** 2.4× headroom worst-case, 4.3× headroom realistic.

## LRU Cache Conclusion

**Not needed for memory management.**

Evidence:
- At 600 tenants, worst-case memory: 720 MB (54% of 1.33 GB headroom)
- Realistic Zipf: 336 MB (25% of headroom)
- LRU adds 4× query latency (513µs vs 129µs) from cache misses
- LRU saves 26 MB at 150 tenants (2% of capacity)

**Actual constraint:** File descriptors. 600 tenants = 1200-3000 FDs. Default ulimit (1024) breaks at 300-400 tenants. Solution: `ulimit -n 65536` in systemd config, not application-level LRU.

## Recommended Architecture

```
TenantRouter with DashMap:
- Lazy-load on first access
- No eviction logic
- Keep all tenants loaded
- OS page cache handles cold data
```

Skip IndexManager entirely. Reactive eviction fallback only if production contradicts tests:
```
if rss_mb() > 1064 { evict_coldest(50) }
```

## Open Risks

1. **Reader metadata scaling:** Tests used 1-segment indexes. Production segment counts may increase per-tenant overhead.

2. **Non-Zipf workloads:** If query distribution is uniform, working set = 1.2 MB × N. Detectable via tenant query rate variance metrics.

3. **Concurrent write spikes:** 10 simultaneous writers = 500 MB transient. Untested in current scope.

4. **Fast field scaling curve:** Unknown if √(docs), log(docs), or other. Tests maxed at 50K docs/tenant.