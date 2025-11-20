
this is rsesearch code.  actual implementaiton is here: 

(venv) stuart@Stuarts-MBP ~/r/flapjack202511 (main)> pwd
/Users/stuart/repos/flapjack202511
(venv) stuart@Stuarts-MBP ~/r/flapjack202511 (main)> 





























<!-- https://claude.ai/chat/cd81bc76-aee0-4d5d-a89a-4d6ea2b387ec -->

<!-- stuart@Stuarts-MBP ~/r/flapjack_rust (main)> fswatch -o /Users/stuart/repos/flapjack_rust | xargs -n1 -I{} ~/sync-to-ec2.sh -->
<!-- stuart@Stuarts-MBP ~/r/flapjack_rust (main)> ~/sync-to-ec2.sh -->

sync stuff:
https://claude.ai/chat/e1101ecd-2d84-42c8-a109-915ed0983dd4

# Tantivy Multi-Tenancy Viability Test

https://claude.ai/chat/cd81bc76-aee0-4d5d-a89a-4d6ea2b387ec

## Quick Start

```bash
cd tantivy_test
cargo run --release
```

**Time required:** 5-10 minutes  
**System requirements:** Linux (for RSS measurement), 8GB+ RAM

## What This Tests

### Test 1: Empty Index Overhead
Creates 100 empty Tantivy indices with minimal writers (5MB each).  
**Measures:** Base memory cost before any data.

### Test 2: Realistic Overhead  
Creates 20 indices with 10K docs each (e-commerce products).  
**Measures:** Memory cost with actual data, indexing speed.

### Test 3: Query Performance
Runs typical queries across 20 indices.  
**Measures:** Query latency when searching multiple tenants.

### Test 4: DocValues Sorting
Tests query-time access to columnar data (your core differentiator).  
**Note:** Full sorting requires custom collector code.

### Test 5: Hot/Cold Pattern
Simulates repeated flushes (mimics your 50MB hot index → cold).  
**Measures:** Commit latency, segment proliferation risk.

## Interpreting Results

### Memory Overhead Decision Tree

**Per-index overhead < 50MB:**
→ ✅ Excellent. Proceed with Tantivy.

**Per-index overhead 50-100MB:**
→ ⚠️ Acceptable if:
- Target 50-70 tenants/machine (not 100)
- Revenue per machine justifies overhead
- Can vertical scale to 128GB+ RAM

**Per-index overhead 100-150MB:**
→ ⚠️ Marginal. Calculate unit economics:
- Cost: $X/GB RAM × overhead
- Revenue: $Y/tenant
- If overhead cost >20% of tenant revenue → build custom

**Per-index overhead >150MB:**
→ ❌ Too high. Either:
- Build custom on LMDB
- Use single shared Tantivy index with tenant_id field (but lose clean backup boundaries)

### Query Latency

**<50ms P99:** ✅ Excellent  
**50-100ms P99:** ✅ Acceptable  
**100-200ms P99:** ⚠️ Borderline (marketing claims "<100ms" won't hold)  
**>200ms P99:** ❌ Not competitive with Algolia/Meilisearch

### Commit Time

**<1s:** ✅ Meets "1s write visibility" claim  
**1-2s:** ⚠️ Acceptable but update marketing to "sub-2s"  
**>2s:** ❌ Too slow for real-time feel

## Expected Results (Rough Estimates)

Based on architecture:
- Empty index overhead: ~20-40MB each
- With 10K docs: ~60-120MB each
- Query latency: 5-50ms depending on result size
- Commit time: 100-500ms

If actual results 2x worse than estimates → reconsider approach.

## Red Flags

**🚩 100 indices consume >20GB RAM:**
→ Overhead too high for target density

**🚩 Query latency increases linearly with index count:**
→ Contention issues, won't scale to 100 tenants

**🚩 Commit time >2s:**
→ Hot/cold architecture won't achieve 1s visibility

**🚩 Test crashes with OOM:**
→ Memory management issues

## Next Steps After Results

### If Tantivy looks good:
1. Week 2: Test DocValues random access performance (requires FFI or custom code)
2. Week 2: Test concurrent writes (multiple threads adding docs to different indices)
3. Week 3: Prototype tenant routing layer
4. Decision: Proceed with 16-week plan

### If Tantivy marginal:
1. Calculate break-even tenant density
2. Decide: Accept lower density OR build custom
3. If custom: prototype LMDB + FST (2 weeks)
4. Timeline extends to 24-28 weeks

### If Tantivy fails:
1. Switch to custom implementation plan
2. Timeline extends to 28-32 weeks
3. Use Tantivy algorithms as reference, not library

## Code Notes

- **RSS measurement works on Linux only** - MacOS/Windows will show warning
- Test creates temp dirs in `/tmp/tantivy_test_*` (~500MB)
- Cleanup happens automatically but you can `rm -rf /tmp/tantivy_test_*` after
- Uses Tantivy 0.22 (stable, production-used)

## Troubleshooting

**"Failed to create index":**
→ Check /tmp is writable and has space

**"Memory measurement not available":**
→ You're on Mac/Windows. Results still useful but can't measure overhead precisely.

**Test hangs:**
→ Probably Test 2 (indexing 200K docs). Give it 5 minutes.

**OOM during test:**
→ Your machine has <8GB RAM. Reduce `num_indices` in test functions.

## Post-Test Analysis

Run this to see total disk usage:
```bash
du -sh /tmp/tantivy_test_*
```

Compare memory vs disk:
- Memory higher than disk = metadata/FST overhead
- Memory ~= disk = mostly mmap'd data
- Memory << disk = good (on-demand loading working)

## Questions This Answers

✅ How much RAM does each Tantivy index consume?  
✅ Can we fit 50-100 indices in 64GB RAM?  
✅ Is query latency acceptable for multi-tenant?  
✅ Does commit time meet 1s write visibility goal?  

⚠️ **Not tested:** True concurrent multi-threaded writes  
⚠️ **Not tested:** Custom DocValues sorting implementation  
⚠️ **Not tested:** Tenant migration (checkpoint → restore)

Those require Week 2-3 prototyping after decision to proceed.