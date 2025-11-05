https://claude.ai/chat/473330f0-98d5-4e8a-888e-e5f72ec5f29e

# Reality Check

## You're asking the wrong question

"Should we build with limits now or design for sharding?" is **premature** because you don't know if your single-file approach even works at the boundary.

**Test you need to run:**
- 1M docs in one LMDB file
- Measure file size, query latency, migration time
- This tells you if single-file scales to "medium" customers (not just tiny ones)

**Why this matters:** If 1M docs = 50GB file and 5-minute migration, your entire tenant-juggling premise breaks. You can't move a customer in 5 minutes when queries are timing out.

---

## The sharding question has one answer

**You WILL need sharding eventually.** Not debatable if you want "enormous enterprise customers."

**Real question:** Do you build it now or retrofit later?

### Build sharding now IF:
- You can't enforce hard limits (customers expect unlimited scale)
- Your migration strategy must support 100GB+ tenants
- You need this working in 6 months

### Defer sharding IF:
- You can cap at 10GB per tenant for initial launch
- 95% of target customers are <100K docs
- You're okay turning away outliers for first year

**The answer depends on your go-to-market timeline, not technical elegance.**

---

## What the research shows

# What The Research Shows

## Competitor Limits

**Algolia:**
- Max index size: 100 GB (1 GB on Build plan)
- Max record size: 10 KB minified JSON
- Max 1,000 indices per application
- Rate limits exist to protect search capacity during heavy indexing

**Meilisearch:**
- Max 4.3 billion docs per index (32-bit unsigned integer limit)
- Recommended max index size: 2 TiB (technically up to 80 TiB on Linux)
- Uses dynamic virtual address space management - starts with 2TB per index, resizes if needed
- Max 20 indexes if each approaches 2TiB due to virtual memory constraints

**Key insight:** Both Algolia and Meilisearch hit fundamental limits. Meilisearch struggled with the trade-off between number of indexes and maximum size, eventually implementing dynamic resizing.

## What This Means For You

### 1. **Single-file-per-tenant WILL hit limits**

Meilisearch uses **the same storage engine as you (LMDB)** and they:
- Had to remove `--max-index-size` parameter before v1.0 stabilization
- Implemented LRU cache for opening indexes
- Accept that large indexes (>2TB) limit concurrent index count

**Your 10GB "test at boundary" is too conservative.** The real boundary is 100GB-2TB based on competitor experience.

### 2. **Multi-region replication has one accepted pattern**

Neither competitor does multi-region writes. Industry standard:
- Primary region for writes
- Async replication to read replicas
- Accept eventual consistency (100-500ms lag)

**Your "seamless global replication" requirement conflicts with LMDB's single-writer model.** You need to either:
- Accept primary-only writes (like competitors)
- Switch storage engines entirely
- Build distributed consensus layer (massive complexity)

### 3. **Sharding is inevitable for enterprise**

Real Meilisearch user: 900MB JSON → 34GB on disk after indexing 300K documents. That's **38x expansion**.

If you assume similar expansion:
- 1M docs × 38x = 38GB minimum
- 10M docs × 38x = 380GB
- 100M docs × 38x = 3.8TB

**Single-file-per-tenant breaks somewhere between 1M-10M docs.**

## My Actual Recommendation

### Do these tests THIS WEEK:

**Test 1: 10GB tenant boundary**
- Index 1M docs with realistic content
- Measure: file size, query P99, migration copy time
- Decision point: If >30 second migration, need sharding

**Test 2: Primary-replica replication prototype**
- Set up LMDB primary + replica
- Stream write log (tail LMDB file? WAL shipping?)
- Measure replication lag
- Decision: Is 100-500ms lag acceptable?

**Test 3: Tenant size distribution research**
- Survey potential customers: median docs, 95th percentile
- Estimate: what % need >10GB?
- Decision: Can you enforce limits or must support unlimited?

### Then choose path:

**Path A: Ship with hard limits**
```
Recommendation:
- Max 10GB per tenant (enforce at API)
- Single-file architecture
- Move large customers to dedicated nodes
- Defer sharding 12+ months

Tradeoffs:
+ Simple architecture
+ Fast to market
+ Tests already validate this works
- Turn away some enterprise customers
- Need migration path later
```

**Path B: Design for sharding now**
```
Recommendation:  
- Tenant = collection of shards
- Add storage abstraction layer NOW
- Query = scatter-gather
- Test at 100GB scale

Tradeoffs:
+ Supports unlimited scale from day 1
+ No migration pain later
- 3-6 months additional development
- Untested complexity
- May be premature optimization
```

## The Strategic Question

**What's your 12-month roadmap?**

If goal is:
- Launch MVP, prove product-market fit → **Path A**
- Win enterprise deals from day 1 → **Path B**

**You can't decide architecture without deciding business strategy.** The 10GB boundary test tells you if Path A is viable. If migration is <10 seconds at 10GB, ship with limits. If >30 seconds, need sharding.

## One More Thing

Algolia recommends: "condense indices using attributes and filters" instead of creating more indices. They discourage one-index-per-tenant architectures.

**But you're doing one-file-per-tenant for migration flexibility.** This is actually a differentiator if you can make it work. The test is: does your migration story remain fast at realistic tenant sizes?

**Run the 10GB test. Everything else is speculation.**