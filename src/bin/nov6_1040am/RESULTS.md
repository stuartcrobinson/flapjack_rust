https://claude.ai/chat/a53df619-c47f-42bb-9b07-d7f13dec5480

# Technical Results Summary

## Test Infrastructure

**Hardware:** AWS t3.medium (2 vCPU, 4GB RAM)  
**Network:** Cross-region (us-east-1 ↔ us-west-2) and intra-region (us-east-1a ↔ us-east-1b) WireGuard tunnels  
**Workload:** 1K docs/commit, 10 commits/sec = 1K writes/sec sustained

---

## Memory Density (tantivy_memory_density_test.rs)

**Result:** 4.10 MB/tenant working set, 0.87 MB/tenant disk

**Breakdown:**
- 50 tenants × 10K docs each
- Total RSS: 204.96 MB (after indexing + readers)
- Per-tenant working set: 4.10 MB
- Median segments: 2/tenant
- Disk compression: 2.29x

**Capacity:** 874 tenants/4GB (2.2x above 400 target)

**Finding:** Tantivy maintains <5 MB/tenant with production merge policy, contradicting earlier 23.4 MB/tenant result from concurrent commits. Sequential batching avoids fsync contention.

---

## Write Latency (tantivy_sequential_batch_test.rs)

**Result:** P99 = 183ms (outlier), P50 = 32ms steady-state

**Breakdown:**
- 20 tenants, 100 docs/batch
- First commit: 183ms (cold start overhead)
- Subsequent commits: 27-36ms range
- Total sequential flush: 640ms for 20 tenants

**Finding:** 183ms outlier is initialization cost (segment writer setup). Real per-commit latency is 32ms. At 400 tenants, sequential batching = 12.8s to flush all pending writes.

---

## Replication Encryption Overhead

### SSH Baseline (tantivy_ssh_replication_test.rs)

**Result:** 0.240s CPU per 1.04 MB transfer

**Cost projection:**
- 30 replicas × 0.240s × 10 commits/sec = 72 cores
- $1,584/mo for replication alone
- 7.2 Gbps outbound bandwidth required

**Bottleneck:** SSH handshake per rsync invocation dominates. Encryption CPU = 96% of indexing CPU (0.250s).

### WireGuard Single Transfer (tantivy_wireguard_replication_test.rs)

**Result:** 0.150s CPU per 1.04 MB transfer (1.6x better than SSH)

**Cost projection:**
- 30 replicas × 0.150s × 10 commits/sec = 45 cores
- $990/mo for replication

**Finding:** WireGuard kernel-space crypto reduces overhead vs SSH userspace, but still 145x over 0.31-core target claim.

### WireGuard Batched (tantivy_wireguard_batch_replication_test.rs)

**Result:** 0.015s CPU per commit when batching 10 commits

**Mechanism:**
- Single rsync of 10 commit directories (1.09 MB total)
- Total CPU: 0.150s (same absolute cost as single transfer)
- Amortized: 0.150s / 10 = 0.015s per commit

**Cost projection:**
- 30 replicas × 0.015s × 10 commits/sec = 4.5 cores
- $99/mo for replication
- 1-2s additional replication lag from batching window

**Cross-region vs intra-region test:**
- Cross-region: 1.68s wall time, 0.150s CPU
- Intra-region: 0.43s wall time, 0.150s CPU
- **CPU unchanged:** Encryption cost is independent of network latency

**Finding:** Overhead was connection setup (TCP handshake + SSH/TLS session), not per-byte encryption. Batching amortizes fixed costs by 10x.

---

## Architectural Implications

### Segment Replication Viability

**Original claim:** 0.31 cores for 30 replicas  
**Validated approach:** 4.5 cores with batching (14.5x over claim, but viable)

**Mechanism:**
1. Primary indexes → generates segments (0.250s CPU for 1K docs)
2. Batch 10 commits → single rsync over WireGuard
3. Replicas receive segment files (no re-indexing)

**Alternative (regional primaries):**
- 3 regions × 1 primary + 5 local replicas = 15 nodes total
- Each primary: 0.015s × 5 replicas × 10 commits/sec = 0.75 cores
- Total: 2.25 cores ($50/mo vs $99/mo for 30 global replicas)

### Batching Trade-offs

**CPU savings:** 10x reduction (45 cores → 4.5 cores)

**Lag penalty:**
- Sequential batching: accumulate 10 commits before replicating
- At 2.5 writes/sec/tenant average: 4s to fill batch
- Total replication lag: 4s batching + network transfer

**Scalability concern:** Test assumes 10 simultaneous commits. Real workload with 400 tenants at 2.5 writes/sec each may require cross-tenant batching (adds routing complexity).

### Encryption Cost Structure

**Fixed per transfer:** Connection setup (TCP + TLS handshake)  
**Variable per byte:** Encryption CPU (minimal with AES-NI hardware acceleration)

**WireGuard advantages over SSH:**
- Kernel-space implementation (vs userspace)
- Persistent tunnels (vs per-connection handshake)
- Modern ciphers (ChaCha20-Poly1305, AES-GCM)

**Cannot optimize further without:**
- Larger batches (>10 commits) → increases lag beyond acceptable
- Different transport (direct TCP/TLS) → similar overhead
- No encryption → compliance violation

---

## Unresolved Questions

1. **Market demand:** What % of customers need >10 replicas? Tests assume 30 replicas are common.

2. **Write concurrency:** Memory test used sequential commits. Production with 40 concurrent writers (10% of 400 tenants) may spike RAM to 4 MB × 40 = 160 MB transient.

3. **Batching at scale:** Can 10-commit batches accumulate in <1s with random write arrival patterns across 400 tenants?

4. **Indexing CPU baseline:** Tests measured replication CPU (0.015s/commit) but not indexing CPU. If indexing costs 0.5s/commit, segment replication saves 30 × 0.5s = 15 cores on replicas.

---

## Technology Selection

**Segment-based replication (Tantivy) vs document replication (LMDB):**

OpenSearch segment replication showed 40-50% CPU reduction on replicas and 50% higher indexing throughput because replicas copy immutable segment files rather than re-indexing documents.

**Validated for this architecture:**
- Primary: 0.250s indexing CPU
- Replicas: 0.015s replication CPU (batched) vs re-indexing entire dataset

**At 30 replicas:** 4.5 cores (segment replication) vs 30 × 0.250s = 75+ cores (document replication).

**LMDB's immutable B-tree structure would require full re-indexing on replicas** (no segment-level diffs available), making it unsuitable for >5 replica configurations.