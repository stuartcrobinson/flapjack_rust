# What You've Actually Validated

## Core Architecture: Proven
- **Segment replication works:** 0.015s/commit with batching vs 0.250s re-indexing
- **At 30 replicas:** 4.5 cores ($99/mo) vs 75+ cores (document replication)
- **Tantivy memory:** 4.10 MB/tenant working set, 874 tenants/4GB capacity
- **Write latency:** 32ms P50 sequential commits (acceptable for search)

## What's Missing

**You haven't measured the thing that justifies segment replication.**

Your entire LMDB vs Tantivy debate hinged on: "segment replication saves CPU on replicas vs document re-indexing."

**But you never measured indexing CPU baseline.**

All your tests measured:
- Replication cost: 0.015s (batched) or 0.150s (single)
- Memory: 4.10 MB/tenant
- Write latency: 32ms

**You didn't measure:** How much CPU does indexing 1K docs cost?

If indexing costs 0.1s, segment replication saves: 30 replicas × 0.1s = 3 cores (marginal vs 4.5-core overhead)
If indexing costs 0.5s, segment replication saves: 30 replicas × 0.5s = 15 cores (validates entire approach)

---

# The Real Question

Based on search results, Tantivy indexes English Wikipedia (5M docs, 8GB) in ~94 seconds, meaning **~50K docs/second sustained**. At 1K docs/commit, that's 0.02s indexing CPU per commit.

Your test showed 0.250s for 1K docs. **That's 12.5x slower than Tantivy's benchmark.**

## The Problem

**Your indexing measurement is contaminated.**

Looking at your test:
```rust
let cpu_before = get_process_cpu_time();
primary.add_documents(doc_count);
primary.commit().unwrap();
let indexing_cpu = get_process_cpu_time() - cpu_before;
// Result: 0.250s
```

This measures: document parsing + tokenization + BM25 scoring + segment building + **commit (fsync)**.

Commit includes fsync, which your test environment showed takes ~0.030s (from sequential batch test: 32ms P50).

**Real indexing CPU: 0.250s - 0.030s = 0.220s**

At 30 replicas, segment replication saves: 30 × 0.220s = **6.6 cores**

That's meaningful but less dramatic than hoped. Segment replication overhead (4.5 cores) eats most of the savings.

## What You Should Do Next

**Stop testing. Start building.**

You've validated:
1. Segment replication works (4.5 cores for 30 replicas)
2. Memory density works (874 tenants/4GB)
3. Write latency acceptable (32ms batched)

**Unknown but not blocking:**
- Query throughput under load (likely fine, test in staging)
- Concurrent write memory spikes (monitor in production)
- Batching accumulation rates (tune after launch)

**You're overthinking because you're avoiding the hard part:** building the actual search engine (BM25, filters, facets, API).

Ship with 10 replicas initially. Test demand for >10 before optimizing for 30. The 4.5-core cost is real but premature—most customers won't need 30 replicas.

**Next 2 weeks:** Build search indexing pipeline and basic query API. Measure real-world performance, not synthetic benchmarks.