# BM25 Memory Test: Decision Framework

## What We're Actually Testing

Your entire 400-tenant-per-node economic model depends on one unvalidated assumption:

**BM25 metadata overhead ≤ 2-3 MB per tenant**

If false, your infrastructure costs double (or worse) and margins shrink.

## The Three Scenarios

### Scenario A: ≤3 MB/tenant (Victory)
**Result:** Architecture validated exactly as designed.

**Actions:**
1. Proceed immediately to query planner (filters + sorts)
2. Build intersection algorithm (Week 3 of roadmap)
3. Maintain 400 tenant density target
4. Target pricing: $1/tenant with 364% margin

**No changes needed.**

---

### Scenario B: 3-5 MB/tenant (Acceptable Compromise)
**Result:** Overhead higher than estimated but still profitable.

**Revised density:** 200-250 tenants per 4GB node
**Infrastructure cost:** $0.12-0.15 per tenant
**Margin at $1/tenant:** 267%-333%

**Actions:**
1. Update capacity planning docs
2. Adjust pricing model slightly (maybe $1.50/tenant for comfort)
3. Proceed with roadmap - still competitive
4. Consider optimization later (not urgent)

**Why acceptable:**
- Algolia charges $1-5/tenant, you're still <50% their cost
- 200 density still gives huge operational advantages
- Multi-tenancy migration benefits unchanged
- Meilisearch charges $30-300/month baseline, you undercut massively

**Trade-offs accepted:**
- Slightly higher per-tenant cost
- May need to monitor tenant sizes more carefully
- Less headroom for future features

---

### Scenario C: >5 MB/tenant (Architecture Risk)
**Result:** BM25 overhead too high for economic model.

**Max density:** 100-150 tenants per node
**Infrastructure cost:** $0.20-0.30 per tenant
**Margin at $1/tenant:** 70%-200% (compressed)

**Critical decisions required:**

#### Option 1: Optimize BM25 Storage
**Approach:** Reduce memory footprint through compression.
- Delta-encode doc_ids in posting lists (save ~40%)
- Varint-encode term frequencies (save ~30%)
- Store doc lengths in compressed batch structure

**Timeline:** 1-2 weeks additional work
**Risk:** May only reduce to 4-5 MB (marginal improvement)

#### Option 2: Two-Tier Metadata Storage
**Approach:** Keep BM25 metadata out of hot memory path.
- Store doc lengths in separate DB, load on-demand
- Accept slower scoring (50ms → 80ms P99)
- Trade memory for latency

**Timeline:** 1 week additional work
**Risk:** May not compete on performance benchmarks

#### Option 3: Reduce Free Tier Limits
**Approach:** Limit documents per tenant to reduce overhead.
- Free tier: 5K docs instead of 10K
- Still competitive with Algolia/Meilisearch limits
- Forces upgrades sooner

**Timeline:** Product decision only
**Risk:** Less appealing to free users

#### Option 4: Accept Lower Density
**Approach:** Build at 100-150 tenants per node.
- Infrastructure: $0.20-0.30/tenant
- Pricing: $2-3/tenant to maintain margins
- Still cheaper than Algolia ($5+/tenant)

**Timeline:** Update projections only
**Risk:** Lower margins, less competitive

**Recommended sequence if Scenario C:**
1. Profile memory allocation (1 day)
2. Identify if posting lists or doc metadata dominates
3. Try Option 1 (compression) first - best ROI
4. If insufficient, consider Option 3 (reduce limits)
5. Option 2 only if performance hit acceptable
6. Option 4 as last resort

---

## Query Performance: Secondary Validation

P99 latency target: **<50ms** (Algolia/Meilisearch advertised)

### If P99 < 30ms:
- ✅ Excellent, headroom for filters/sorts
- Proceed with multi-field sort + filter intersection

### If P99 30-50ms:
- ✅ Acceptable, but tight budget
- Must carefully optimize intersection algorithm
- May need Roaring bitmaps for large posting lists

### If P99 > 50ms:
- ⚠️ Not competitive on performance
- **Root causes:**
  - Posting list iteration too slow (optimize decoding)
  - BM25 scoring expensive (cache IDF values)
  - Top-k sorting inefficient (use heap)
- **Solution:** Profile and optimize before proceeding

---

## The Critical Path Forward

**After memory test completes:**

1. **Document actual numbers** in project doc
2. **Decide scenario** (A/B/C)
3. **Update roadmap** based on scenario:
   - Scenario A → Week 3 as planned
   - Scenario B → Week 3 + adjust pricing docs
   - Scenario C → 1-2 week optimization sprint first

**Don't proceed to query planner if Scenario C and >10 MB/tenant.**
You'd be building on a broken economic foundation.

---

## Why This Test Matters More Than Others

Your previous tests validated:
- ✅ LMDB per-env overhead (0.048 MB) 
- ✅ Sequential commits (4.18ms P99)
- ✅ Concurrent reads (0.017ms P99)
- ✅ Migration copy (38ms for 57 MB)

All excellent. But those are **infrastructure** validations.

This test validates **product economics**.

If BM25 metadata is 10 MB/tenant:
- All your infrastructure tests still pass
- Migration still works
- LMDB still performs well
- **But the business model breaks**

You'd need to charge $3-5/tenant to match Algolia's margins, losing your competitive advantage.

---

## What "Good" Looks Like

**Ideal test output:**

```
=== SUMMARY ===
Memory overhead per tenant:
  Open (passive):  0.15 MB
  Working set:     2.40 MB
  Total per tenant: 2.55 MB  ← THIS NUMBER

Capacity projection (4GB node):
  Available RAM: 3584 MB
  Max tenants: 405

✅ PASS: 2.55 MB/tenant meets <3 MB target
   400 tenant density: VIABLE
```

**If you see this, pop champagne.** Your architecture is validated.

**If you see 4-6 MB:** Totally workable, adjust projections.

**If you see 10+ MB:** Stop and debug before building more features.

---

## Timeline After Tests

**Scenario A (≤3 MB):** 
- Day 1: Run tests, document results
- Day 2-3: Build FST term index (already designed)
- Day 4-7: Build query planner with filters
- Week 2: Multi-field sorts + intersection algorithm

**Scenario B (3-5 MB):**
- Day 1: Run tests, document results  
- Day 2: Update projections and pricing
- Day 3+: Same as Scenario A timeline

**Scenario C (>5 MB):**
- Day 1: Run tests, document results
- Day 2-3: Profile memory, identify bottleneck
- Day 4-7: Implement compression or redesign
- Day 8: Re-test and validate improvement
- Day 9+: Resume Week 3 roadmap if fixed

---

## Questions the Test Answers

1. **Can we fit 400 tenants/node?** 
   → Yes if ≤3 MB, no if >5 MB

2. **Is BM25 metadata the bottleneck?**
   → Compare to baseline (0.048 MB env overhead)

3. **Does working set grow with query load?**
   → Watch RSS during Phase 5 heavy queries

4. **Are posting lists or doc metadata dominant?**
   → Disk usage shows posting list size
   → Memory delta shows hot pages

5. **Can we compete on performance?**
   → P99 latency vs 50ms target

6. **Is the architecture sound?**
   → Only if Scenario A or B

---

## Final Note

You've done excellent infrastructure validation work. This is the last critical assumption to validate before building the actual search features.

**Run the tests. Get the number. Then decide.**

Don't guess on 2-3 MB/tenant. Measure it.
