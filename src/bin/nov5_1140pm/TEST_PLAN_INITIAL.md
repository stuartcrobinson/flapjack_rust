# Flapjack Testing Plan

## Priority 1: Validate Core Replication Assumptions (Blocking)

### Test 1: Segment Replication Lag
**Question:** Can async segment replication keep standby <2s behind primary under load?

**Method:**
```rust
// Primary A: Index 1K docs/sec for 60s
// Measure: B's index lag (timestamp of last synced segment)
// Target: P99 lag <2s, P50 <500ms
```

**Pass:** Architecture viable  
**Fail (>5s lag):** Need sync replication (adds 10-20ms write latency) or accept data loss on failover

### Test 2: Segment Generation Overhead
**Question:** Does generating segments for standby add >50ms to commit latency?

**Method:**
```rust
// Baseline: commit without segment export
// Test: commit + generate segments + rsync to standby
// Measure: P99 latency delta
// Target: <50ms added overhead
```

**Pass:** 2x primary cost justified  
**Fail:** May need single primary + cold standby (5-30min failover)

### Test 3: 30-Replica rsync Load
**Question:** Can primary handle 30 concurrent rsync connections without CPU spike?

**Method:**
```rust
// Primary commits, 30 replicas pull simultaneously
// Measure: Primary CPU usage during rsync storm
// Target: <20% CPU overhead
```

**Pass:** Pull model scales  
**Fail:** Need rate limiting or push queuing

## Priority 2: Failover Mechanics (Critical Path)

### Test 4: Health Check False Positive Rate
**Question:** Does 5s health check interval cause false failovers under normal load?

**Method:**
```rust
// Run A+B for 24 hours under variable load
// Induce: CPU spikes, network congestion, GC pauses
// Measure: False positive failover triggers
// Target: 0 false positives
```

**Pass:** 5s interval acceptable  
**Fail:** Tune interval or add multi-check confirmation

### Test 5: Replica Coordination Time
**Question:** How long until all 30 replicas switch to new leader after failover?

**Method:**
```rust
// Kill A, B promotes
// Measure: Time until last replica queries etcd + rsyncs from B
// Target: <10s for 90% of replicas
```

**Pass:** Fits 99.9% budget  
**Fail:** Need push notifications or reduce poll interval

### Test 6: Split-Brain Detection
**Question:** What happens if A and B both think they're leader?

**Method:**
```rust
// Network partition between A and B
// Both write to different clients
// Verify: Fencing tokens prevent dual-write
// Target: One shuts down cleanly
```

**Pass:** Data integrity preserved  
**Fail:** Need external lock service (ZooKeeper pattern)

## Priority 3: Global Network Resilience

### Test 7: Rsync Through Bad Networks
**Question:** Can replicas in China/Africa maintain <10s lag despite packet loss?

**Method:**
```rust
// Simulate: 10% packet loss, 1000ms latency, intermittent timeouts
// Measure: Replica lag under sustained degradation
// Target: Catches up within 30s of network recovery
```

**Pass:** Pull model self-heals  
**Fail:** Need protocol fallback (HTTPS over CDN, delta compression)

### Test 8: Firewall Traversal
**Question:** Does rsync work through Great Firewall / corporate proxies?

**Method:**
```rust
// Deploy test replica behind China proxy
// Attempt rsync with various ports/protocols
// Measure: Success rate
```

**Pass:** rsync viable  
**Fail:** Switch to HTTPS segment delivery via S3/CDN

## Priority 4: Density & Cost Validation

### Test 9: 400-Tenant Memory Under Replication
**Question:** Does segment generation for standby increase primary's memory footprint?

**Method:**
```rust
// Index 400 tenants with segment replication enabled
// Measure: RSS vs baseline (no replication)
// Target: <10% increase (still fits 4GB)
```

**Pass:** Density target maintained  
**Fail:** Reduce tenant density or disable standby for free tier

### Test 10: Multi-Region Cost Reality Check
**Question:** What % of writes go to tenants with >10 replicas?

**Method:**
```
// Competitor analysis: Survey Algolia/Meilisearch pricing tiers
// Calculate: Revenue/cost ratio at various replica counts
// Validate: "30 replicas common" assumption
```

**Pass:** High-replica customers exist and pay premium  
**Fail:** Over-optimized for rare case, LMDB cost difference irrelevant

## Non-Blocking (Defer to Phase 2)

### Test 11: etcd Single-Node Failure
Measure: Failover delay if etcd down (replicas use stale leader). Acceptable for 99.9%.

### Test 12: Concurrent Tenant Failovers
Question: Can 100 tenants failover simultaneously without etcd overload?

### Test 13: Segment Merge During Replication
Question: Do background segment merges cause rsync thrashing?

## Decision Gates

**After Test 1-2:** If either fails, 2-primary architecture invalid. Revert to single primary + cold standby.

**After Test 3:** If rsync load too high, implement push notifications + pull hybrid.

**After Test 5-6:** If coordination too slow or split-brain unresolvable, add 3rd node for quorum.

**After Test 7-8:** If rsync fails globally, redesign replication transport (S3-based or HTTPS).

**After Test 10:** If high-replica demand unproven, simplify to 1-3 replica architecture.

## Timeline Estimate

- Priority 1 (replication core): 3-5 days
- Priority 2 (failover): 3-4 days  
- Priority 3 (global networks): 2-3 days
- Priority 4 (validation): 2-3 days

**Total: 10-15 days to validate or invalidate architecture.**

Run Priority 1 first. If Tests 1-2 fail, entire design needs revision.

https://claude.ai/chat/39fc6aee-7c0f-47bf-95c3-60b2eacbddc4