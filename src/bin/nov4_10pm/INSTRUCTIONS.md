# BM25 Implementation & Memory Tests

## Setup

1. **Add dependencies to `Cargo.toml`:**

```toml
[dependencies]
heed = "0.20"
anyhow = "1.0"
serde = { version = "1.0", features = ["derive"] }
rand = "0.8"
byteorder = "1.5"
tempfile = "3.8"  # for tests
```

2. **Add module to your project:**

Create `src/bm25.rs` from the provided file, or add as a module in your existing structure.

3. **Add test binaries to `Cargo.toml`:**

```toml
[[bin]]
name = "bm25_memory_test"
path = "src/bin/bm25_memory_test.rs"

[[bin]]
name = "bm25_correctness_test"
path = "src/bin/bm25_correctness_test.rs"

[[bin]]
name = "bm25_query_perf_test"
path = "src/bin/bm25_query_perf_test.rs"
```

4. **Create the bin directory structure:**

```bash
mkdir -p src/bin
```

5. **Copy test files:**

Place the three test files in `src/bin/`.

6. **Add module reference in each test file:**

Each test file starts with `mod bm25;` - this assumes `bm25.rs` is at `src/bm25.rs`.
If your structure is different, adjust the module path.

## Running Tests

### Test 1: Memory Overhead (CRITICAL)

This measures the actual RSS overhead per tenant with BM25 metadata.

```bash
cargo run --release --bin bm25_memory_test
```

**Expected output:**
- Per-tenant memory breakdown
- Verdict on 400 tenant density
- If >3 MB/tenant, density projections

**What to look for:**
- Total memory per tenant (passive + working set)
- If <3 MB: ✅ Architecture validated
- If 3-5 MB: ⚠️ Acceptable, reduces density to 200-250
- If >5 MB: ❌ Need to optimize or redesign

### Test 2: Correctness

Validates BM25 scoring against known test cases.

```bash
cargo run --release --bin bm25_correctness_test
```

**Expected:** All assertions pass, correct ranking order.

### Test 3: Query Performance

Benchmarks query latency across different workload patterns.

```bash
cargo run --release --bin bm25_query_perf_test
```

**Expected:** P99 < 50ms for all query types (to compete with Algolia/Meilisearch).

## Interpreting Results

### Critical Decision Point: Memory Test Results

**Scenario A: <3 MB/tenant** (Target)
- ✅ Proceed with 400 tenant density
- Infrastructure cost: $0.075/tenant
- Margin at $1/tenant pricing: 364%

**Scenario B: 3-5 MB/tenant** (Acceptable)
- ⚠️ Reduce density to 200-250 tenants/node
- Infrastructure cost: $0.12-0.15/tenant
- Margin at $1/tenant pricing: 267%-333%
- Still competitive with Algolia

**Scenario C: >5 MB/tenant** (Problematic)
- ❌ Need optimization or architecture change
- Options:
  1. Store BM25 metadata separately (slower scoring)
  2. Compress posting lists more aggressively
  3. Reduce free tier document limits
  4. Accept lower density (100 tenants = $0.30/tenant)

### Performance Targets

Based on competitor benchmarks:
- Algolia: <50ms response time advertised
- Meilisearch: <50ms response time advertised

Your P99 latency should be <50ms for:
- Single-term queries
- Multi-term queries (2-4 terms)
- Top-100 retrieval

If P99 >50ms:
- Profile the hot path (likely posting list intersection)
- Consider Roaring bitmaps for large posting lists
- Optimize scoring loop

## Next Steps After Tests

1. **If memory test passes (<3-5 MB):**
   - ✅ Proceed to filters + sort implementation
   - Build query planner with intersection
   - Measure combined text+filter+sort P99

2. **If memory test fails (>5 MB):**
   - Analyze breakdown: posting lists vs doc metadata
   - Profile actual memory allocations
   - Consider compression (varint, delta encoding)
   - Test with smaller document sets

3. **If performance test fails (P99 >50ms):**
   - Profile with `perf` or `flamegraph`
   - Optimize posting list iteration
   - Consider index optimizations (skip lists, etc)
   - May need Roaring bitmaps for intersections

## Troubleshooting

**Error: "module not found: bm25"**
- Ensure `src/bm25.rs` exists
- Or adjust `mod bm25;` to correct path

**Error: "/proc/self/status not found"**
- You're not on Linux. Replace `get_rss_mb()` with platform-specific code
- Or remove RSS measurements (just run for timing/correctness)

**Slow indexing:**
- Expected for debug builds
- Always use `--release` flag
- 50K docs should index in <10s on modern hardware

**High memory usage during test:**
- Expected - we're opening 20 tenants simultaneously
- Watch for memory growth during query phase
- That's the "working set" we're measuring

## What the Tests Measure

### Memory Test
1. **Baseline RSS:** Process overhead before any LMDB
2. **Open overhead:** Memory for LMDB handles (minimal)
3. **Working set:** Pages faulted during queries (the critical metric)
4. **Disk usage:** Actual LMDB file size

The working set is what limits tenant density. It includes:
- BM25 doc length metadata (u32 per doc)
- Posting list pages accessed during queries
- LMDB B-tree internal nodes

### Query Performance Test
1. **Single-term:** Baseline BM25 scoring
2. **Multi-term:** Union + scoring overhead
3. **Rare terms:** Small posting lists (fast)
4. **Common terms:** Large posting lists (slow)
5. **Top-k:** Sorting/heap overhead

This reveals:
- If posting list size affects P99
- If BM25 scoring is the bottleneck
- If top-k retrieval scales

## Expected Runtime

- Memory test: ~30-60 seconds (indexes 200K docs)
- Correctness test: <1 second
- Query perf test: ~20-40 seconds (indexes 50K docs, runs 5K queries)

Total: ~2 minutes to validate BM25 architecture.
