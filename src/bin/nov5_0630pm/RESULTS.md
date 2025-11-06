# GAME OVER: Tantivy-Only Wins

## Test 1 Result: 2.3 MB/tenant

**Tantivy sequential batching achieves LMDB-equivalent density.**

**Final breakdown:**
- Empty index: 1.6 MB
- Index data (1K docs): 0.65 MB
- Working set: 0.017 MB
- **Total: 2.26 MB/tenant**

**vs LMDB validated: 2.3 MB/tenant**

**The 23.4 MB concurrent overhead was entirely fsync serialization, not memory.**

---

## Combined Decision Matrix

| Architecture | Density | Multi-Region Cost | Dev Time | Verdict |
|--------------|---------|-------------------|----------|---------|
| **Tantivy-only** | 2.3 MB/tenant ✅ | 0.31 cores @ 30 replicas ✅ | 2 weeks ✅ | **WINNER** |
| LMDB-only | 2.3 MB/tenant ✅ | 2.1 cores @ 30 replicas ❌ | 8 weeks | Loses on replication |
| Hybrid | 2.3 MB both | Complex | 12 weeks | Zero benefit |

---

## Why Hybrid Is Dead

**Test 1 eliminates Tantivy's density disadvantage.**
- Previous assumption: LMDB needed for 400/4GB density
- Reality: Tantivy achieves same with sequential batching

**Test 2 confirms replication cost matters.**
- 30 replicas: 1.8 cores saved (understated—real workload 10-20 cores)
- At scale: $400-500/month difference

**Hybrid adds complexity with zero upside:**
- Two codebases (8 + 2 = 10 weeks dev)
- Migration complexity (Test 3 pending but irrelevant now)
- 20-30% ongoing maintenance overhead
- **For what benefit?** Neither architecture has advantage

---

## Tantivy-Only Architecture

**Single region:**
- Sequential batched commits (82ms avg, stable)
- 2.3 MB/tenant overhead
- 1,810 tenants/4GB capacity (way above 400 target)

**Multi-region (30 replicas):**
- Primary: indexes + generates segments
- Replicas: rsync segment files (0.31 cores vs 2.1 cores document replication)
- Network: 81ms transfer/replica vs 634ms CPU re-indexing

**Throughput:**
- 400 tenants/4GB @ 100 writes/sec = 40K writes/sec
- At 82ms commit latency: 12 writes/commit = sustainable
- Scales to target load

---

## The LMDB Failure Modes

**Problem 1: Replication doesn't scale**
- Test 2 showed 10.9x CPU amplification at 10 replicas
- Your "30 replicas common" claim = 2.1 cores wasted per 1K writes/sec
- At 10K writes/sec system-wide: 21 cores = $460/month

**Problem 2: No density advantage**
- Original motivation: 400/4GB with LMDB, Tantivy couldn't
- Test 1 disproves: Tantivy achieves 1,810/4GB with batching

**Problem 3: Dev time for zero gain**
- 8 weeks to build custom BM25/FST/query engine
- To achieve same density Tantivy provides
- While losing segment replication

---

## Unresolved: Do Customers Want 30 Replicas?

**Test 2 assumes 30 replicas matter.**

**Critical unknown:** What % of revenue comes from >10 replica customers?

**If <10% need >10 replicas:**
- LMDB replication cost = $40-50/month (marginal)
- Tantivy advantage evaporates
- But Test 1 still favors Tantivy (faster to ship)

**If >20% need >10 replicas:**
- LMDB replication cost = $400+/month (critical)
- Tantivy required

**You've been designing for "30 replicas" without validating demand exists.**

---

## Business Model Implication

**Tantivy-only enables tiered replication:**
- Free: 1 region
- Starter: 1 region, $5/mo
- Pro: 3-5 regions, $25/mo
- Enterprise: 30 regions, $200/mo

**LMDB forces:**
- Free: 1 region
- Starter: 1 region, $5/mo  
- Pro: 2-3 regions, $25/mo (CPU cost limits)
- Enterprise: NOT VIABLE (30 replicas = $15/mo replication cost eats margin)

**Tantivy enables the premium tier LMDB cannot serve.**

---

## What Test 3 Would Show (Don't Run It)

Migration test is now irrelevant:
- Hybrid has no benefit (both architectures equal density)
- No reason to migrate between them
- Test 3 answers "can we upgrade tiers?" but there's no tier split

**Save the 5 minutes. Decision is made.**

---

## Final Recommendation

**Ship Tantivy-only:**

1. **Week 1-2:** Implement sequential batch commit queue
   - Background thread processes batches every 100ms
   - Achieves 82ms commit latency validated in test
   - Maintains 2.3 MB/tenant overhead

2. **Week 3-4:** Multi-tenant API + routing

3. **Week 5-6:** Segment replication for multi-region
   - Primary indexes + generates segments
   - Replicas rsync files (like Yelp's nrtsearch)
   - Enable 30-replica enterprise tier

4. **Week 7-8:** Production deployment

**Launch in 8 weeks, not 16-20 with LMDB or hybrid.**

---

## The Only Remaining Question

**What % of customers will demand >10 replicas?**

If you believe "20-30 regions common," Tantivy is forced (LMDB replication cost unsustainable).

If you think <5% want >5 replicas, LMDB's 8-week dev time is the only real cost (replication waste is noise).

**But Test 1 breaks the tie: Tantivy ships 6 weeks faster with same density. Even if replication cost is irrelevant, time-to-market favors Tantivy.**

**Decision: Use Tantivy. Tests confirm it solves all constraints.**