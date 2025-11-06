https://claude.ai/chat/5b212596-1465-44dc-807a-c0a79cbc4024

# Tantivy Multi-Tenant Architecture Validation Tests

## Context
Previous LMDB tests validated cross-tenant atomic commits via single-file multi-DB architecture. Tantivy uses separate index directories per tenant, requiring different validation approach.

Critical unknowns after architecture pivot:
1. Does sequential batching avoid fsync serialization catastrophe (P99 = 3,851ms with 10 concurrent tenants)?
2. What's actual memory overhead per tenant with optimized merge policy?
3. Does segment replication actually save CPU vs document replication?

## Tests

### 1. `tantivy_sequential_batch_test.rs`
**Question:** Can sequential batching per-tenant avoid write contention?

**Method:**
- 20 tenants, separate Tantivy directories
- Sequential commits: collect 100 docs/tenant, commit one-by-one
- Measure P99 latency, compare to your catastrophic concurrent result

**Success:** P99 <100ms per tenant (2 sec total for 20 tenants = acceptable)
**Failure:** Still >500ms per tenant = Tantivy write path fundamentally broken

### 2. `tantivy_memory_density_test.rs`
**Question:** What's per-tenant memory overhead with production-ready merge policy?

**Method:**
- 50 tenants × 10K docs each
- Configure merge policy: max 5 segments per index (prevent explosion)
- Measure RSS after indexing, during queries, with inactive tenants

**Success:** <3 MB/tenant working set = 400 tenants/4GB viable
**Failure:** >5 MB/tenant = density target drops to 200/4GB

### 3. `tantivy_segment_replication_cost.rs`
**Question:** Does copying segment files avoid re-indexing CPU cost?

**Method:**
- Primary: index 10K docs, measure CPU time
- Replica: rsync segment directory, measure CPU time
- Compare: full re-index vs segment copy

**Success:** Segment copy <10% of indexing CPU = your 0.31 core claim validated
**Failure:** Segment copy requires index rebuild = replication cost model wrong

## Expected Results
- Test 1: Sequential batching should work (your concurrent test methodology likely flawed)
- Test 2: Overhead likely 8-15 MB/tenant (worse than LMDB but acceptable)
- Test 3: Segment replication should win decisively (this is why Quickwit uses it)

## Decision Gates
**If Test 1 fails:** Tantivy unusable, revert to LMDB or accept queue-based write delays
**If Test 2 shows >5 MB/tenant:** Reduce density target to 200/4GB, update economics
**If Test 3 fails:** Global replication cost model broken, recalculate infrastructure costs


https://claude.ai/chat/c236cb4f-f5a7-4d51-bcd2-dff50125a07d
https://claude.ai/chat/6c27cad2-0668-4ac0-9f29-4e889b397f3e
https://claude.ai/chat/78699abc-1e2b-4c3c-8f99-3139b2a50f42