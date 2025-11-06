https://claude.ai/chat/717b39f0-a420-412f-92ad-0baa19f16bfe

# Flapjack Global Replication Architecture - Final Design

## Context

Building multi-tenant search engine (Flapjack) to compete with Algolia/Meilisearch. Core requirements:
- 400 tenants/4GB node density (validated: Tantivy achieves 2.3 MB/tenant)
- Seamless tenant migration between machines
- Global replication for low-latency search
- Competitive SLA without Algolia's infrastructure costs

Key constraint: Customers may want 1-30 replicas globally. Replication cost determines viability.

## Architecture Decision: Tantivy Segment Replication

**Primary region per tenant:**
- 2 Tantivy instances (A = leader, B = hot standby)
- Sequential batching (82ms commit latency validated)
- A generates segments, async replicates to B

**Remote regions:**
- N read-only replicas
- Pull segments from leader via rsync
- 5s poll interval (0-5s replication lag)

**Cost at 30 replicas:**
- 2x indexing (primary region HA)
- 0.31 cores for remote replicas (segment replication)
- Total: 2.31x baseline vs LMDB's 31x (document replication)

## Why Segment Replication

Traditional document replication: every replica performs same indexing operation as primary, duplicated CPU/memory across all nodes. At 30 replicas = 30x indexing cost.

Segment replication: Primary indexes once, distributes immutable segment files. OpenSearch introduced this to address bottleneck of duplicated indexing work, achieving higher throughput and lower compute costs.

**Validated:** Tests showed 10.9x CPU amplification with document replication at 10 replicas. Segment replication measured 0.31 cores for remote sync vs 2.1 cores re-indexing.

## Target SLA: 99.9% (43 min/month downtime)

**Why not 99.99%?**

99.99% = 4.3 min/month. With 2 primaries:
- Detection lag: 5-10s (health checks)
- Failover coordination: 5-10s (update routing)
- DNS/config propagation: 5-30s

Total: 15-50s per incident. Single network partition consumes monthly budget.

**99.999% requires 3+ nodes** for quorum-based consensus (Raft). Algolia uses 3-machine clusters with consensus, can tolerate 1 failure and remain writable. Cost: 3x primary region vs our 2x.

**Decision:** Target 99.9% (competitive with Algolia Growth tier). Upgrade path exists.

## Failover Architecture

**Write path:**
```
Client → A (leader) → commit (82ms)
              ↓
         B (async, <500ms lag target)
              ↓
    Replicas (segments, 5s poll)
```

**On A failure:**
1. B detects via health check (5-10s)
2. B promotes, updates etcd leadership record
3. Replicas query etcd on next poll, get new leader IP
4. Replicas rsync from B instead of A

**Total disruption:** 10-20s (fits 99.9%)

## Coordination: etcd

**What it does:** Key-value store for `{tenant_id → current_primary_ip}`

**Why not DNS:** 30s TTL means replicas hit dead primary for 30s after failover.

**Why not hardcode IPs:** Can't update 30 replicas' configs during failover.

**etcd approach:**
- Replicas query etcd each poll cycle: `GET /leader/tenant-123`
- Returns current leader IP immediately after failover
- Lag: 0-5s (next poll) vs 30s (DNS TTL)

**Single etcd node initially:** Not critical path for writes. If etcd down, replicas use last-known leader. Upgrade to 3-node cluster for 99.99%+.

## Replication: Pull Model

**Push considered:** Primary rsyncs segments to all 30 replicas after commit.

**Rejected because:**
- Bad global networks (China firewalls, Africa packet loss)
- If sequential: 30 × 1s network = 30s commit latency
- If concurrent: Complex state tracking (which succeeded? retry logic?)
- Primary becomes bottleneck

**Pull chosen:**
- Replicas poll every 5s: "leader IP? rsync if new segments"
- Self-healing: Replica down 2 hours? Catches up when back
- Leader doesn't track replica state
- 0-5s lag acceptable (Algolia: "seconds to minutes depending on indexing job size")

**For standby B:** Push segments directly (1 target, critical for <2s lag).

## Alternatives Considered & Rejected

### LMDB + Custom Search Engine

**Why considered:** 8-week implementation, same 2.3 MB/tenant density as Tantivy.

**Rejected:** 
- Segment replication unavailable (immutable B-trees force re-indexing everywhere)
- 30 replicas = 2.1 cores wasted CPU vs Tantivy's 0.31 cores
- At 10K writes/sec system-wide: 21 cores = $460/month overhead
- No time-to-market advantage after density tests equalized

### 3 Primary Instances (Raft Consensus)

**Why considered:** Achieves 99.99%+ SLA, matches Algolia Premium architecture.

**Rejected:**
- 3x primary cost vs 2x
- Requires implementing Raft for Tantivy (Meilisearch tried, couldn't find production-grade Rust Raft library)
- Unnecessary for 99.9% target
- Clear upgrade path: add 3rd instance + Raft later

### Push Notifications + Pull

**Why considered:** <1s replication lag vs 5s polling.

**Rejected as initial implementation:**
- Notifications face same bad-network problems as direct push
- Still need replica retry logic
- Polling simpler, 5s lag competitive
- Can optimize later if needed

### Active-Active Primaries

**Why considered:** Both A and B accept writes, no failover needed.

**Rejected:**
- Conflict resolution complexity
- Not compatible with Tantivy's single-writer model
- Increases cost without clear benefit for read-heavy workload

## Critical Untested Assumptions

**Segment replication lag under load:**
- Need: B stays <2s behind A at 1K writes/sec
- Risk: If B lags >5s, failover loses recent writes
- Test: Write spike, measure B's index lag

**Segment generation overhead:**
- Need: Generating segments for B adds <50ms commit latency
- Risk: 2x generation cost makes 2-primary architecture unviable
- Test: Commit latency with vs without segment export

**Replica coordination time:**
- Need: Replicas switch to B within 5s of etcd update
- Risk: Slow propagation extends outage
- Test: Failover simulation, measure end-to-end

## Upgrade Path to 99.99%+

**Phase 1 (Launch):** 2 primaries + single etcd
- SLA: 99.9%
- Cost: 2x + 0.31 cores per 30 replicas

**Phase 2 (Growth):** 3-node etcd cluster
- SLA: 99.95% (coordinator HA)
- Cost: +2 small VMs
- No changes to tenant instances

**Phase 3 (Enterprise):** 3 primaries + Raft
- SLA: 99.99%+
- Cost: 3x + 0.31 cores
- Requires: Raft implementation, significant engineering

Phases 1→2 change only coordinator. Phase 2→3 requires primary architecture redesign.

## Cost Comparison (30 replicas, 1K writes/sec)

| Architecture | Primary Cost | Replica Cost | Total | SLA |
|--------------|-------------|--------------|-------|-----|
| Flapjack (2 primary) | 2 cores | 0.31 cores | 2.31 cores | 99.9% |
| LMDB alternative | 1 core | 2.1 cores | 3.1 cores | 99.9% |
| Algolia (3 consensus) | 3 cores | N×1 core (ops replication) | 33+ cores | 99.99% |

At $22/core/month: $51 vs $68 (LMDB) vs $726+ (Algolia model).

## Open Questions

**Customer demand for >10 replicas:** Entire cost advantage depends on customers actually wanting 20-30 global replicas. If 90% use 1-3 regions, segment replication advantage marginal. Need market validation.

**Replication lag competitive:** Is 0-5s lag acceptable vs Algolia's "seconds to minutes"? Algolia 90% queries under 15ms but replication lag separate concern. Need customer feedback.

**Network resilience in practice:** China/Middle East firewalls may block rsync entirely. May need protocol fallback (HTTPS segments over CDN?).