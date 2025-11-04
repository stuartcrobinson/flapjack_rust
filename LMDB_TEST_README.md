# LMDB Multi-Tenancy Viability Test

## Setup Instructions

### 1. Add files to your project

```bash
cd /Users/stuart/repos/flapjack_rust

# Create bin directory
mkdir -p src/bin

# Copy the test binary
cp /tmp/lmdb_test_binary.rs src/bin/lmdb_test.rs

# Update Cargo.toml (or manually add dependencies)
# Add to [dependencies] section:
#   lmdb = "0.8"
#   rand = "0.8"
```

### 2. Update Cargo.toml

Add this to your `Cargo.toml`:

```toml
[[bin]]
name = "lmdb_test"
path = "src/bin/lmdb_test.rs"

[dependencies]
lmdb = "0.8"
rand = "0.8"
```

### 3. Run the test

```bash
cargo run --release --bin lmdb_test
```

**Time:** 5-10 minutes  
**Note:** Memory measurement only works on Linux. Mac will show warnings but tests still run.

## What This Tests

### Test 1: Empty Database Overhead
- Creates 100 empty LMDB databases in single environment
- **Measures:** Base memory cost of multi-database setup
- **Critical:** LMDB designed for this pattern (100s of named DBs)

### Test 2: Realistic Inverted Index
- 20 databases × 10K documents
- Builds actual inverted index (term → posting list)
- Uses delta encoding (production pattern)
- **Measures:** Memory with real search data

### Test 3: Query Performance  
- Term lookups across 20 databases
- **Measures:** Latency for your core operation (search)
- Compare with Tantivy's query times

### Test 4: Columnar Storage
- Separate databases for price/category fields
- Simulates DocValues (your query-time sort feature)
- **Measures:** Random access time (critical for sorting)

### Test 5: Write Performance
- Simulates your hot→cold flush (5K terms per flush)
- **Measures:** Transaction commit latency
- Target: <1s to meet "1s write visibility"

### Test 6: Migration Pattern
- Simulates moving tenant from Machine A → B
- **Measures:** How long to copy database files
- LMDB advantage: Simple file copy vs RocksDB checkpoint complexity

## Decision Matrix

### Memory Overhead Per Database

| Result | Action |
|--------|--------|
| <20MB | ✅ **Ideal** - Proceed with LMDB |
| 20-50MB | ✅ **Good** - Lower than Tantivy, viable |
| 50-100MB | ⚠️ **Acceptable** - Similar to Tantivy but more work |
| >100MB | ❌ **Too high** - No advantage over Tantivy |

### Query Latency

| Result | Action |
|--------|--------|
| <100μs | ✅ **Excellent** - Sub-millisecond |
| 100-1000μs | ✅ **Good** - ~1ms, acceptable |
| 1-10ms | ⚠️ **Marginal** - Acceptable but not competitive edge |
| >10ms | ❌ **Too slow** - Uncompetitive |

### Write Transaction Time

| Result | Action |
|--------|--------|
| <500ms | ✅ **Excellent** - Meets 1s visibility goal |
| 500ms-1s | ✅ **Good** - Meets goal with buffer |
| 1-2s | ⚠️ **Marginal** - Update marketing to "sub-2s" |
| >2s | ❌ **Too slow** - Breaks real-time feel |

## Compare with Tantivy

After running both tests:

| Metric | LMDB Target | Tantivy Expected |
|--------|-------------|------------------|
| Overhead/DB | <50MB | 60-120MB |
| Query latency | <1ms | 5-50ms |
| Write commit | <500ms | 100-500ms |
| Implementation | 6-8 weeks | 2 weeks |
| FST/BM25 | Build yourself | Built-in |
| Control | Full | Library constraints |

## Critical Questions

### 1. Is overhead advantage real?
If LMDB = 30MB/tenant vs Tantivy = 80MB/tenant:
- 100 tenants: 3GB vs 8GB (5GB savings)
- Does 5GB savings justify 6 weeks extra dev?
- Calculate: $5/GB cloud RAM × 5GB = $25/month per machine
- Is that meaningful given your revenue model?

### 2. Query performance gap?
LMDB gives raw KV lookups. You build:
- Posting list intersection (AND queries)
- BM25 scoring
- Result ranking

Tantivy has optimized implementations. If LMDB queries 5x slower, does lower overhead matter?

### 3. Development timeline risk
LMDB path requires building:
- FST for term dictionary (2 weeks)
- Posting list compression (1 week)  
- BM25 scorer (1 week)
- Query parser (2 weeks)
- Segment merging (2 weeks)

That's 8 weeks assuming no setbacks. Tantivy: 2 weeks.

## Expected Results (Rough)

Based on architecture:
- Empty DB overhead: ~5-15MB each
- With 10K docs: ~30-60MB each (vs Tantivy 60-120MB)
- Query latency: 100-1000μs (vs Tantivy 5-50ms)
- Write commit: 100-500ms (similar to Tantivy)

**If LMDB 2x better on memory but 3x worse on queries → is that a good trade?**

## Red Flags

🚩 **100 databases consume >10GB RAM**  
→ Overhead not better than Tantivy

🚩 **Query latency >10ms average**  
→ Your custom implementation too slow

🚩 **Write commit >2s**  
→ Can't meet 1s write visibility claim

🚩 **Migration test fails**  
→ Tenant movement story unclear

## Next Steps

### If LMDB Wins (lower overhead, acceptable queries):

1. **Week 1-2:** Build FST term dictionary
   - Use `fst` crate as foundation
   - Test: 1M terms → FST size and lookup time
   
2. **Week 3:** Posting list compression
   - Delta encoding + varint
   - Test: 100K posting lists → compression ratio
   
3. **Week 4:** BM25 scoring
   - Standard formula, test against Tantivy results
   
4. **Week 5-6:** Query parsing and execution
   - Boolean queries (AND/OR/NOT)
   - Phrase queries
   
5. **Week 7-8:** Segment merging strategy
   - Tiered compaction (like RocksDB/Lucene)
   
Timeline: 8 weeks to basic search engine, then add multi-tenancy.

### If Tantivy Wins (overhead acceptable, queries fast):

1. **Week 1-2:** Tantivy integration tests (already done)
2. **Week 3:** Multi-tenant routing layer
3. **Week 4:** Migration protocol
4. **Week 5-8:** Hot/cold architecture on Tantivy

Timeline: 8 weeks to working MVP.

### If Neither Wins:

Reconsider architecture entirely:
- Single shared Tantivy index with tenant_id field?
- RocksDB database-per-tenant after all?
- Commercial search engine (Elastic/Meilisearch) + optimization layer?

## Post-Test Analysis

Check disk usage:
```bash
du -sh /tmp/lmdb_test_*
```

If memory >> disk → high metadata overhead (bad)  
If memory ≈ disk → mostly mmap (good)  
If memory << disk → excellent on-demand loading

## What This Doesn't Test

⚠️ **Multi-threaded writes** - Production needs concurrent writes  
⚠️ **Compaction strategy** - How to prevent segment explosion  
⚠️ **Real BM25 scoring** - Only tests raw term lookups  
⚠️ **FST overhead** - You'd need to build FST to measure actual overhead

These need Week 2-3 prototyping.

## The Real Question

**Is building a custom search engine worth 6-8 weeks to save 50MB per tenant?**

Math:
- 100 tenants × 50MB savings = 5GB
- Cloud RAM: $5/GB/month = $25/month savings
- Dev cost: 6 weeks × $10K/week (blended rate) = $60K
- Break-even: 2400 months = 200 years

Unless:
1. You need <50MB/tenant for unit economics
2. You need custom data structures Tantivy can't provide
3. You value full control over library dependency

Otherwise: Use Tantivy, ship faster, iterate on product-market fit.

## Recommendation

Run both tests. Calculate actual overhead difference. Decide based on data, not speculation.

Then ask: **Does the overhead difference matter for your business model?**


-------------

I'll create an LMDB multi-tenancy test that matches the Tantivy test structure and integrates into your existing project.

Now I'll create the necessary files to integrate this into your project structure:

Let me create the proper project structure for you:

I'll create files that you can drop into your existing project structure:

Now let me create a README for the LMDB test:

Perfect! Here's what I've created for you:

## Files Created

1. **`/tmp/Cargo.toml`** - Updated with LMDB dependencies and binary config
2. **`/tmp/lmdb_test_binary.rs`** - The complete LMDB test suite
3. **`/tmp/LMDB_TEST_README.md`** - Setup instructions and decision framework

## Integration Steps

```bash
cd /Users/stuart/repos/flapjack_rust

# Create bin directory
mkdir -p src/bin

# Copy test file
cp /tmp/lmdb_test_binary.rs src/bin/lmdb_test.rs

# Update your Cargo.toml to add:
# [[bin]]
# name = "lmdb_test"
# path = "src/bin/lmdb_test.rs"
#
# [dependencies]
# lmdb = "0.8"
# rand = "0.8"
```

Then run:
```bash
cargo run --release --bin lmdb_test
```

## What This Tests vs Tantivy

| Test | LMDB | Tantivy | Critical Difference |
|------|------|---------|---------------------|
| Empty overhead | 100 named DBs in 1 env | 100 separate indices | LMDB designed for this |
| With data | Custom inverted index | Built-in FST/posting lists | You build vs library |
| Query speed | Raw KV lookup | Optimized search structures | Likely slower |
| Migration | File copy | Checkpoint API | Simpler |
| Dev time | 6-8 weeks | 2 weeks | 3-4x longer |

## The Real Questions

1. **Memory advantage meaningful?** If LMDB = 30MB/tenant vs Tantivy = 80MB/tenant, does 50MB × 100 tenants = 5GB savings justify 6 weeks extra dev?

2. **Query performance penalty acceptable?** LMDB will be slower (no FST, no optimized posting list intersection). If 3-5x slower, does memory saving matter?

3. **Business model dependent?** If you charge $99/month/tenant and 5GB RAM costs $25/month, the savings is irrelevant. If you charge $9/month/tenant and margins are 20%, it's critical.

## My Take

**LMDB test will likely show:**
- 2-3x better memory overhead
- 3-5x worse query performance  
- Similar write performance

**The decision depends on:**
- Your actual pricing model (you haven't shared)
- Whether query speed or density matters more
- Risk tolerance (custom code vs battle-tested library)

**Most likely outcome:** Tantivy overhead is acceptable when amortized over revenue, and you ship 6 weeks faster.

**Run both tests.** Decide with data, not vibes.

Questions:
1. What's your target price per tenant?
2. What's your target machine spec (RAM/CPU)?
3. How many tenants per machine affects unit economics?