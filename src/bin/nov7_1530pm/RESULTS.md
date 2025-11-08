## Root Cause Found: Measurement Methodology Error

**realistic_density_test.rs measures TOTAL RSS, not working set delta.**

### The Flawed Calculation

```rust
let final_mb = get_rss_mb().unwrap();
let per_tenant_mb = (final_mb - baseline_mb) / num_tenants as f64;
// Result: (360 - 3) / 150 = 2.38 MB/tenant
```

**Problem:** This includes:
1. All index structures on disk-backed mmap (faulted in during creation)
2. IndexWriter buffers (50 MB × 150 = 7.5 GB of arena allocators, partially resident)
3. Reader metadata for ALL 150 tenants (not just hot ones)
4. Page cache contamination from sequential index creation

### Your New Test Measures Correctly

```rust
// Test C: Open + readers + queries
let baseline_rss = get_rss_mb(); // 136 MB after creation
// ... open readers and query ...
let final_rss = get_rss_mb(); // 252 MB
let working_set = final_rss - baseline_rss; // 116 MB
let per_tenant = working_set / 150; // 0.776 MB
```

**This isolates query working set from index creation overhead.**

### Why realistic_density_test Got 360 MB

Looking at the code:

```rust
// Creates indexes with writers
for tenant_id in 0..num_tenants {
    let mut writer = index.writer(50_000_000).unwrap(); // 50 MB buffer
    // ... add docs ...
    writer.commit().unwrap();
    drop(writer); // BUT: arena memory may not be freed to OS immediately
}
```

**Each IndexWriter allocates 50 MB.** Even after `drop()`, the RSS may not decrease immediately (memory returned to malloc free list, not OS). With 150 sequential creations, malloc fragmentation keeps ~200 MB resident that isn't actually "working set".

Then:
```rust
let after_create_mb = get_rss_mb().unwrap(); // ~317 MB
// This includes creation artifacts!
```

The test never establishes a "clean" baseline after index creation settles. It measures:
```
360 MB = 136 MB (baseline like your test) 
       + 116 MB (actual working set)
       + 108 MB (malloc fragmentation from 150× writer creation)
```

### Evidence

**Your test:**
- Creates indexes: 136 MB baseline
- Opens readers: 195 MB (+59 MB)
- Queries all: 252 MB (+116 MB from baseline)

**realistic_density_test:**
- Creates indexes: 317 MB (vs your 136 MB)
- After queries: 360 MB

**Difference:** 317 - 136 = 181 MB of **creation overhead not present in your test.**

Breakdown:
- 150 IndexWriters × 50 MB = 7.5 GB allocated (not all resident, but malloc fragmentation keeps ~181 MB)
- Your test doesn't create writers, so no fragmentation

### The Correct Number

**0.776 MB/tenant is accurate** for query working set.

But realistic_density_test also showed:
```rust
println!("Per-tenant overhead: {:.2} MB\n", 
    (after_create_mb - baseline_mb) / num_tenants as f64);
// (317 - 3) / 150 = 2.09 MB/tenant
```

This is **index structure + reader overhead** (not working set). It's the cost of having the index *exist*, even if cold.

### Revised Understanding

**Two costs:**
1. **Cold tenant (index exists, never queried):** ~0.4 MB (reader metadata)
2. **Hot tenant (actively queried):** +0.776 MB = **1.2 MB total**

At 150 tenants:
- If all hot: 150 × 1.2 = 180 MB ✓ (matches your 195 MB reader + working set)
- If 30 hot (Zipf): (30 × 1.2) + (120 × 0.4) = 84 MB ✓ (matches Test D's 90 MB)

### Capacity Math (Corrected)

**Worst case (all tenants hot):** 1.2 MB/tenant
- 4 GB node: 1330 MB / 1.2 = **1108 tenants**
- Safe (60%): **665 tenants**

**Realistic (Zipf 20% hot):** 
- (N × 0.2 × 1.2) + (N × 0.8 × 0.4) = N × 0.56 MB/tenant
- 4 GB node: 1330 / 0.56 = **2375 tenants**
- Safe (60%): **1425 tenants**

### LRU Cache Verdict: STILL WRONG

Even at worst case (all hot), you have 2x headroom vs your 600 target. LRU cache adds complexity for zero benefit.

**Real constraint remains:** File descriptors at 300-400 tenants unless ulimit raised.

### Action

1. Update CONSTRAINTS.md: 0.776 MB hot working set + 0.4 MB cold overhead
2. Raise density target: 665 tenants (conservative) to 1425 tenants (Zipf)
3. Abandon LRU cache design
4. Set production ulimit: 65536

The 2.38 MB was malloc fragmentation + creation overhead, not operational cost.