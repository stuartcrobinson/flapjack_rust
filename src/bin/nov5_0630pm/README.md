https://claude.ai/chat/d0d80d4b-70dd-4cc1-aefb-e24487de8627

# Critical Architecture Decision Tests

## Purpose
Answer blocking question: **LMDB-only, Tantivy-only, or hybrid?**

## Tests

### 1. `tantivy_batch_density_test`
**Question:** Can Tantivy achieve 400 tenants/4GB with sequential batched writes?

**Why:** Previous test showed 23.4 MB/tenant under concurrent commits. If sequential batching reduces to 8.5 MB (single-threaded baseline), LMDB's density advantage disappears.

**Decision criteria:**
- <10 MB/tenant → Use Tantivy-only (enables segment replication)
- 10-15 MB → Test hybrid (marginal density difference)
- >15 MB → LMDB required

**Run:** `cargo run --release --bin tantivy_batch_density_test`

---

### 2. `lmdb_replication_cost_test`
**Question:** How expensive is LMDB document replication at 10-30 replicas?

**Why:** Claim "30x CPU waste" never measured. Need actual cost vs segment replication.

**Decision criteria:**
- <1 core saved → Document replication acceptable
- 1-5 cores → Marginal, depends on customer distribution
- >5 cores → Segment replication required

**Run:** `cargo run --release --bin lmdb_replication_cost_test`

---

### 3. `lmdb_tantivy_migration_test`
**Question:** Is tier-upgrade migration (LMDB → Tantivy) viable?

**Why:** Hybrid architecture requires customers to migrate when adding regions. If migration takes >5min or breaks queries, hybrid is dead.

**Decision criteria:**
- <3min + queries match → Hybrid viable
- 3-10min → Marginal, depends on customer tolerance
- >10min OR queries broken → Hybrid not viable

**Run:** `cargo run --release --bin lmdb_tantivy_migration_test`

---

## Decision Matrix

Run all three tests, then:

| Test 1 Result | Test 2 Result | Test 3 Result | Recommendation |
|---------------|---------------|---------------|----------------|
| <10 MB/tenant | N/A | N/A | **Tantivy-only** |
| >15 MB/tenant | <1 core | <3min + match | Hybrid viable but complex |
| >15 MB/tenant | >5 cores | Any | **LMDB-only** (accept no multi-region) |
| 10-15 MB | >5 cores | <3min + match | Hybrid worth complexity |

## Expected Runtime
- Test 1: 5-10 minutes (400 indices)
- Test 2: 2-3 minutes (10 replicas)
- Test 3: 3-5 minutes (10K docs)

Total: ~15 minutes for complete decision data.
