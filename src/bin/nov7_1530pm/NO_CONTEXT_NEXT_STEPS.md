## Do Next: Skip IndexManager, Build Core Search API

### Why Skip IndexManager

**Problem it was solving:** Bound memory via LRU eviction of 600 tenants → 120 hot

**Reality:** 
- Worst case all-hot: 1.2 MB/tenant × 600 = 720 MB (54% of 1.33 GB headroom)
- Realistic Zipf: 0.56 MB/tenant × 600 = 336 MB (25% of headroom)
- **No memory pressure exists**

**FD limits:** Solvable with `ulimit -n 65536` in systemd unit file, not application code.

**Cost of IndexManager:**
- 4x query latency (513µs with churn vs 129µs pre-loaded)
- LRU miss = disk seek + reader creation per query
- Complexity: eviction logic, metrics tracking, cache coherency

**Benefit:** Saves 26 MB (2% of capacity) at 150 tenants.

### Alternative Architecture

```rust
struct TenantRouter {
    tenants: DashMap<TenantId, Arc<TenantIndex>>,
}

struct TenantIndex {
    index: Index,
    reader: IndexReader,
    created_at: Instant,
}

impl TenantRouter {
    fn get_or_load(&self, id: TenantId) -> Arc<TenantIndex> {
        self.tenants.entry(id).or_insert_with(|| {
            let index = Index::open_in_dir(path).unwrap();
            let reader = index.reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into().unwrap();
            Arc::new(TenantIndex { index, reader, created_at: Instant::now() })
        }).clone()
    }
}
```

No eviction. No LRU. Just lazy-load on first access, keep forever. OS page cache handles cold data automatically.

### Tests to Run

**None required for MVP.** Your constraints are validated:

✅ Per-tenant cost: 0.776 MB hot, 0.4 MB cold  
✅ 600 tenants fits in 4 GB with 50%+ headroom  
✅ Query latency <200µs sustained  
✅ Zipf distribution keeps working set bounded  

**Post-MVP tests (after you have users):**

1. **Uniform query distribution edge case**  
   - If real workload isn't Zipf, working set = 1.2 MB × N
   - Detectable via metrics: query rate variance across tenants
   - Mitigation: reduce density or add query result cache

2. **Large tenant behavior (100K+ docs)**  
   - Your tests maxed at 50K docs/tenant
   - If working set scales non-linearly at 1M docs, could blow assumptions
   - Mitigation: enterprise tenants get dedicated nodes

3. **Concurrent write memory spikes**  
   - IndexWriter has 50 MB buffer
   - 10 simultaneous writers = 500 MB transient spike
   - Test in Phase 2.2 when you build write path

### Build Order (Revised)

**Week 3: Tenant Router + Search API**
- `TenantRouter` with DashMap (no LRU)
- `POST /tenants` - create index
- `POST /tenants/{id}/search` - query with filters/sort
- `POST /tenants/{id}/documents` - stage writes
- `POST /tenants/{id}/commit` - flush to disk

**Week 4: Faceting + Migration**
- Query-time facet aggregation (validated viable)
- `GET /tenants/{id}/export` - tar tenant directory
- `POST /tenants/import` - extract + register
- Test migration <1s total (38ms copy validated)

**Skip entirely:**
- IndexManager
- LRU cache
- Metrics-driven eviction
- Per-tenant memory tracking

### Risk

**What if I'm wrong and memory does blow up in production?**

**Fallback:** Add lazy eviction trigger at 80% memory threshold:
```rust
if get_rss_mb() > 1064 { // 80% of 1.33 GB
    evict_coldest_tenants(num=50);
}
```

Reactive eviction when pressure exists, not proactive LRU churn. Simpler, zero cost until needed.

### Decision Point

Do you have data contradicting the tests? (Expected query rates, doc sizes, tenant count targets that differ from benchmarks?)

If no: **Build the simple router, ship Week 3 scope.**  
If yes: **What's the discrepancy?**