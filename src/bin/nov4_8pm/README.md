# Tenant Migration Architecture Tests (Nov 4, 8pm)

## Context

After validating LMDB write batching performance, we realized **cross-tenant atomic commits conflict with tenant migration**. The "dynamic juggling" requirement (move tenants between machines) is more important than batching all tenants into one fsync.

**Architecture decision:** Separate LMDB file per tenant (not shared environment with named DBs).

These tests validate whether separate-file-per-tenant is viable for high-density hosting.

---

## Tests

### 1. `separate_env_memory_test.rs`
**Question:** What's the RSS overhead of opening 400 separate LMDB environments?

**Why critical:** If each environment costs 10+ MB overhead just to keep open, 400 tenants = 4GB baseline before any data loads. Pricing model collapses.

**Method:**
1. Create 400 tenant LMDB files (1000 docs each, small corpus to isolate overhead)
2. Measure baseline RSS
3. Open all 400 environments (read-only)
4. Measure RSS delta = per-environment overhead
5. Query 1 tenant → measure active tenant working set
6. Query 10 tenants → measure multi-tenant working set

**Success criteria:**
- Per-environment overhead <1 MB (preferably <0.5 MB)
- Active tenant working set <2 MB
- Total for 400 tenants @ 20% active <3.5 GB (leaves headroom in 4GB)

**Failure modes:**
- Per-env overhead >1 MB → separate files not viable for density target
- Active working set >5 MB → can't support 80 concurrent active tenants

**Run:**
```bash
cargo run --release --bin separate_env_memory_test
```

---

### 2. `sequential_commit_test.rs`
**Question:** Is fsync serialization a problem if commits aren't concurrent?

**Why critical:** Previous Tantivy test showed 3,851ms P99 with 10 concurrent commits. But with batched writes (1 commit/sec per tenant), commits are sequential. Need to validate sequential doesn't serialize.

**Method:**
1. Create 20 separate LMDB environments
2. Batch writes per tenant (50 writes/commit, simulating 1-sec accumulation)
3. Commit tenants SEQUENTIALLY (not concurrently)
4. Measure total time and per-tenant P99
5. Compare to CONCURRENT commits (anti-pattern)

**Success criteria:**
- 20 sequential commits in <200ms (avg <10ms each)
- P99 per-tenant <15ms
- Concurrent commits show penalty vs sequential (validates batching strategy)

**Failure modes:**
- Sequential commits >500ms → LMDB overhead beyond fsync is problematic
- No penalty for concurrent → original fsync theory wrong, LMDB has other bottleneck

**Run:**
```bash
cargo run --release --bin sequential_commit_test
```

**Variants tested:**
- Test 1: All 20 tenants active (worst case)
- Test 2: Only 25% tenants active (realistic sparse writes)
- Test 3: Concurrent commits for comparison (shows penalty of not batching)

---

### 3. `migration_copy_test.rs`
**Question:** Can we copy LMDB file while serving reads/writes? How long?

**Why critical:** "Dynamic juggling" requires moving tenants between machines with minimal downtime. Need to validate:
1. Copy duration for realistic tenant sizes
2. File consistency during copy (LMDB MVCC should handle this)
3. Whether writes during copy corrupt destination

**Method:**
1. Create tenant with 100K docs (~100-500 MB)
2. Background thread: continuous writes at 10/sec
3. Foreground: filesystem copy of entire LMDB directory
4. Verify copied file opens and has correct doc count
5. Compare to `mdb_copy -c` (compact) if available

**Success criteria:**
- Copy <30 sec for 100-500 MB
- Copied DB is consistent (has all pre-copy docs)
- Copied DB is readable and functional
- No corruption from concurrent writes

**Failure modes:**
- Copy >60 sec → too slow for live migration
- Copied DB corrupted → need write pause or mdb_copy instead of filesystem copy
- High write rate during copy causes issues

**Run:**
```bash
cargo run --release --bin migration_copy_test
```

**Note:** Requires `mdb_copy` utility for Phase 5 (optional). Install with:
```bash
apt-get install lmdb-utils  # Debian/Ubuntu
brew install lmdb           # macOS
```

---

## Decision Tree

```
Run Test #1 (separate_env_memory_test)
├─ Per-env overhead >1 MB?
│  └─ YES → ❌ Separate files NOT viable. Reconsider architecture.
│  └─ NO → Continue to Test #2
│
Run Test #2 (sequential_commit_test)
├─ Sequential commits >500ms for 20 tenants?
│  └─ YES → ❌ Write latency too high. LMDB may not scale.
│  └─ NO → Continue to Test #3
│
Run Test #3 (migration_copy_test)
├─ Copy takes >60 sec OR corrupted?
│  └─ YES → ⚠️  Need write pause during migration or use mdb_copy
│  └─ NO → ✅ Architecture validated!
```

---

## Expected Results

**Best case:**
- Per-env overhead: 0.3 MB
- Sequential commits: 100ms for 20 tenants
- Migration copy: 5-10 sec, consistent

**Realistic case:**
- Per-env overhead: 0.5-0.8 MB
- Sequential commits: 150-200ms for 20 tenants
- Migration copy: 15-20 sec, occasional need for write pause

**Failure case:**
- Per-env overhead: >2 MB
- Sequential commits: >1 sec for 20 tenants
- Migration copy: >60 sec or corruption

---

## Architecture Implications

### If all tests pass:
**Confirmed architecture:**
- One LMDB file per tenant
- Sequential commit batching (1/sec per tenant)
- Hot copy for tenant migration
- 400 tenants/4GB viable

**Next steps:**
- Test concurrent read scaling (10K QPS)
- Implement query planner (text + filter + sort)
- BM25 metadata overhead measurement

### If Test #1 fails:
**Reconsider:**
- Hybrid: small tenants share environment, large tenants separate files
- Accept lower density (200 tenants/4GB instead of 400)
- Go back to shared environment with key prefixes + custom export tooling

### If Test #2 fails:
**Reconsider:**
- LMDB may not be viable for high-density multi-tenant
- Look at RocksDB, SQLite, or other embedded DBs
- Accept higher latency or reduce tenant density

### If Test #3 fails:
**Reconsider:**
- Implement write pause during migration (requires coordination)
- Use `mdb_copy` instead of filesystem copy (slower but safer)
- Accept that migrations are slow (minutes of downtime)

---

## Open Questions Not Covered by These Tests

1. **Concurrent read scaling:** How does LMDB handle 4K QPS from 400 tenants?
2. **File descriptor limits:** Does 400 environments × 2 FDs = 800 FDs cause issues?
3. **Tenant size limits:** Performance at 10 GB per tenant (current max before sharding)
4. **Global replication:** How to replicate writes across regions with LMDB's single-writer constraint

These can be tested later after confirming basic architecture viability.