
https://claude.ai/chat/39fc6aee-7c0f-47bf-95c3-60b2eacbddc4

# Segment Generation Overhead Research

## What the Internet Says

**OpenSearch segment replication architecture** (closest analog):
- Primary generates segments during refresh (default 1s interval)
- Segment generation is **part of normal indexing**, not additional overhead
- Cost: fsync + file creation (~10-50ms depending on segment size)
- Replication: rsync/copy segments to replicas (async, doesn't block commits)

**Tantivy architecture** (from docs/issues):
- `IndexWriter.commit()` generates immutable segments
- Segment generation = serialize in-memory buffer to disk (~5-20ms for 1K docs)
- **Already included in your 82ms P99** - you can't commit without generating segments
- No "dual generation" penalty - same segments go to disk and standby

**Yelp's nrtsearch** (Lucene-based, similar to Tantivy):
- Primary commits, replicas copy segment files
- No mention of commit latency increase for replication
- Replication is file copy after commit completes

## Analysis

**Your Test 2 question is malformed.** 

Tantivy doesn't "generate segments for B" as separate work. Commit flow:
1. Write 100 docs to in-memory buffer
2. `commit()` → serialize buffer to segment file (already measured: 82ms)
3. Segment file exists on disk
4. **Separately:** rsync/copy file to B (async, doesn't block step 2)

**There is no "segment generation overhead for standby."** The overhead is copying files, which you measure in Test 1 (replication lag).

**Test 2 should actually measure:** Does enabling replication (having B pull segments) increase A's commit latency due to:
- File locking contention during rsync?
- Disk I/O saturation (A writing, B reading same disk)?
- Network stack overhead?

**Expected answer:** <5ms increase if async. If sync (wait for B to copy), +network latency (50-200ms).

---

# Standby Replication Lag Research

## What the Internet Says

**OpenSearch segment replication performance** (AWS blog):
- Replication lag: <1s P99 at 10K docs/sec indexing rate
- Segment size: 50-500 MB (your 1K docs ≈ 2-5 MB)
- Network: 10 Gbps between AZs
- Bottleneck: network transfer time + fsync on replica

**Lucene/Elasticsearch NRT replication:**
- Near-real-time (NRT) lag: 1-5s typical
- Affected by: segment size, network bandwidth, replica disk speed
- Not affected by: indexing rate (segments queue up, don't block primary)

**MySQL binlog replication** (comparable workload):
- Single-threaded: 1-10s lag typical
- Parallel replication: <1s lag at 10K TPS
- Determinant: network + replay speed

## Analysis

**Your 1K writes/sec = ~10-20 MB/sec segment generation** (assuming 10-20 KB/doc indexed).

**Network math:**
- 1 Gbps LAN: 125 MB/s theoretical, ~100 MB/s real
- Transfer time: 10-20 MB = 100-200ms
- **Lag dominated by: when B polls + transfer time**

**With 5s poll interval** (from GLOBAL_REPLICATION.md):
- Best case: 0s (B polls right after segment created)
- Worst case: 5s (B just missed polling)
- Average: 2.5s lag
- Plus transfer: +0.2s
- **Expected: 0.2-5.2s lag, average 2.7s**

**Your <2s target requires:**
- Poll interval: 1-2s (not 5s)
- Or: Push notification + pull (B pulls immediately when notified)
- Or: Streaming replication (B tails segment files)

---

# Test Plan

## Test 1: Replication Lag (Required)

**Hypothesis:** B stays <2s behind A at 1K writes/sec with 5s polling.

**Method:**
```rust
// Primary A: Index 100 docs/sec for 60s (6K docs total)
// Each doc has timestamp field
// Standby B: Polls every 5s, rsyncs segments
// Measure: timestamp of last doc in B vs current time
// Repeat 10 times for P99
```

**Metrics:**
- P50 lag (expected: 2.5s)
- P99 lag (target: <5s, ideal: <2s)
- Max lag (should never exceed 5s + transfer time)

**Pass criteria:** P99 <5s (fits 99.9% SLA)
**Ideal:** P99 <2s (allows tighter SLA)

**If fails (>10s):**
- Reduce poll interval to 2s
- Add push notifications
- Check: disk I/O saturation? Network congestion?

## Test 2: Commit Latency with Replication (Lower Priority)

**Hypothesis:** Enabling B to rsync doesn't increase A's commit latency.

**Method:**
```rust
// Baseline: A commits, no B
// Test: A commits, B rsyncs simultaneously
// Measure: A's commit latency P99 delta
// 1K iterations each
```

**Metrics:**
- Baseline P99: 82ms (from previous test)
- With replication P99: target <100ms
- Delta: target <20ms

**Pass criteria:** <20ms overhead
**Ideal:** <5ms overhead

**If fails (>50ms):**
- Disk contention (A write + B read same SSD)
- Solution: Separate disks or async with buffering

---

# Recommendations

**Test 1 is critical.** Your 5s poll interval likely causes 2.5-5s average lag, which:
- Fits 99.9% SLA (10-20s failover tolerance)
- Might not fit 99.95% SLA (5-10s tolerance)
- Customers expect "seconds" lag (acceptable)

**Test 2 is less critical** because:
- Segment generation isn't "extra work" for replication
- Async file copy shouldn't block commits
- If disk contention exists, it's an infrastructure fix (separate volumes)

**Priority:** Run Test 1 first. If P99 <5s, architecture validated. If >5s, tune polling or add push notifications (2-3 days work).

**Time estimate:** 4-6 hours to implement both tests.