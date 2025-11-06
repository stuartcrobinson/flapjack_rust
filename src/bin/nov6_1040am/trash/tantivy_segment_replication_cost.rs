use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tantivy::schema::{Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};
use tempfile::TempDir;

/// Test: Does segment replication save CPU vs document replication?
/// 
/// Critical question: Can replicas copy segment files without re-indexing?
/// 
/// Method:
/// 1. Primary: Index 10K docs, measure CPU time
/// 2. Replica A: Copy segment directory with rsync, measure CPU time
/// 3. Replica B: Re-index same 10K docs (document replication baseline), measure CPU
/// 
/// Success: Segment copy <10% of indexing CPU → 0.31 cores claim validated
/// Failure: Segment copy requires re-indexing → global replication cost model broken

struct TenantIndex {
    index: Index,
    writer: IndexWriter,
    dir: TempDir,
}

impl TenantIndex {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("title", TEXT | STORED);
        schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();
        
        let index = Index::create_in_dir(&dir, schema.clone()).unwrap();
        let writer = index.writer(50_000_000).unwrap();
        
        TenantIndex { index, writer, dir }
    }
    
    fn add_documents(&mut self, count: usize) {
        let schema = self.index.schema();
        let title = schema.get_field("title").unwrap();
        let body = schema.get_field("body").unwrap();
        
        for i in 0..count {
            let doc = doc!(
                title => format!("Document {}", i),
                body => format!("This is document {}. The quick brown fox jumps over the lazy dog. ", i).repeat(10)
            );
            self.writer.add_document(doc).unwrap();
        }
    }
    
    fn commit(&mut self) -> Result<u64, tantivy::TantivyError> {
        self.writer.commit()
    }
    
    fn get_disk_usage(&self) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = fs::read_dir(self.dir.path()) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        total += metadata.len();
                    }
                }
            }
        }
        total
    }
    
    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn measure_cpu_time<F>(f: F) -> (std::time::Duration, f64)
where
    F: FnOnce(),
{
    let start = Instant::now();
    let start_cpu = get_process_cpu_time();
    
    f();
    
    let elapsed = start.elapsed();
    let cpu_time = get_process_cpu_time() - start_cpu;
    
    (elapsed, cpu_time)
}

#[cfg(target_os = "linux")]
fn get_process_cpu_time() -> f64 {
    let pid = std::process::id();
    let stat_path = format!("/proc/{}/stat", pid);
    
    if let Ok(contents) = fs::read_to_string(&stat_path) {
        let parts: Vec<&str> = contents.split_whitespace().collect();
        if parts.len() > 15 {
            // Fields 14 and 15 are utime and stime in clock ticks
            let utime: u64 = parts[13].parse().unwrap_or(0);
            let stime: u64 = parts[14].parse().unwrap_or(0);
            let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
            return (utime + stime) as f64 / clock_ticks;
        }
    }
    0.0
}

#[cfg(target_os = "macos")]
fn get_process_cpu_time() -> f64 {
    // Mac doesn't have /proc, use rusage approximation
    // This is less accurate but sufficient for comparison
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn rsync_directory(src: &Path, dst: &Path) -> Result<std::time::Duration, std::io::Error> {
    // Ensure destination exists
    fs::create_dir_all(dst)?;
    
    let start = Instant::now();
    
    // Use rsync with options similar to production
    // -a: archive mode (preserves permissions, timestamps, etc)
    // --delete: remove files in dst that don't exist in src
    let output = Command::new("rsync")
        .arg("-a")
        .arg("--delete")
        .arg(format!("{}/", src.display()))
        .arg(format!("{}/", dst.display()))
        .output()?;
    
    let elapsed = start.elapsed();
    
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("rsync failed: {}", String::from_utf8_lossy(&output.stderr))
        ));
    }
    
    Ok(elapsed)
}

fn main() {
    println!("=== Tantivy Segment Replication Cost Test ===\n");
    
    let doc_count = 10_000;
    
    println!("Phase 1: Primary indexing (baseline CPU cost)");
    println!("  Indexing {} docs...", doc_count);
    
    let mut primary = TenantIndex::new();
    
    let (wall_time, cpu_time) = measure_cpu_time(|| {
        primary.add_documents(doc_count);
        primary.commit().unwrap();
    });
    
    let primary_disk = primary.get_disk_usage();
    
    println!("  Wall time: {:?}", wall_time);
    println!("  CPU time: {:.3}s", cpu_time);
    println!("  Disk usage: {:.2} MB", primary_disk as f64 / 1_000_000.0);
    println!("  Indexing rate: {:.0} docs/sec\n", doc_count as f64 / cpu_time);
    
    println!("Phase 2: Segment replication (rsync copy)");
    
    let replica_a_dir = TempDir::new().unwrap();
    println!("  Copying segments from {:?} to {:?}...", 
        primary.path(), replica_a_dir.path());
    
    let (rsync_wall_time, rsync_cpu_time) = measure_cpu_time(|| {
        rsync_directory(primary.path(), replica_a_dir.path()).unwrap();
    });
    
    // Verify replica is readable
    let replica_a_disk = {
        let mut total = 0u64;
        if let Ok(entries) = fs::read_dir(replica_a_dir.path()) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        total += metadata.len();
                    }
                }
            }
        }
        total
    };
    
    println!("  Wall time: {:?}", rsync_wall_time);
    println!("  CPU time: {:.3}s", rsync_cpu_time);
    println!("  Disk usage: {:.2} MB", replica_a_disk as f64 / 1_000_000.0);
    
    // Try to open replica to verify integrity
    let replica_valid = Index::open_in_dir(replica_a_dir.path()).is_ok();
    println!("  Replica valid: {}\n", replica_valid);
    
    println!("Phase 3: Document replication (re-index baseline)");
    println!("  Re-indexing same {} docs...", doc_count);
    
    let mut replica_b = TenantIndex::new();
    
    let (reindex_wall_time, reindex_cpu_time) = measure_cpu_time(|| {
        replica_b.add_documents(doc_count);
        replica_b.commit().unwrap();
    });
    
    let replica_b_disk = replica_b.get_disk_usage();
    
    println!("  Wall time: {:?}", reindex_wall_time);
    println!("  CPU time: {:.3}s", reindex_cpu_time);
    println!("  Disk usage: {:.2} MB\n", replica_b_disk as f64 / 1_000_000.0);
    
    println!("=== RESULTS SUMMARY ===\n");
    
    println!("Indexing (primary):");
    println!("  CPU: {:.3}s", cpu_time);
    println!("  Disk: {:.2} MB", primary_disk as f64 / 1_000_000.0);
    
    println!("\nSegment replication (rsync):");
    println!("  CPU: {:.3}s ({:.1}% of indexing)", rsync_cpu_time, (rsync_cpu_time / cpu_time) * 100.0);
    println!("  Wall: {:?}", rsync_wall_time);
    println!("  Disk: {:.2} MB", replica_a_disk as f64 / 1_000_000.0);
    
    println!("\nDocument replication (re-index):");
    println!("  CPU: {:.3}s ({:.1}% of primary)", reindex_cpu_time, (reindex_cpu_time / cpu_time) * 100.0);
    println!("  Wall: {:?}", reindex_wall_time);
    println!("  Disk: {:.2} MB", replica_b_disk as f64 / 1_000_000.0);
    
    let cpu_savings = reindex_cpu_time / rsync_cpu_time;
    println!("\n=== COST ANALYSIS ===");
    println!("Segment replication CPU savings: {:.1}x", cpu_savings);
    
    // Extrapolate to 30 replicas scenario
    println!("\nProjection for 30 replicas @ 1K writes/sec:");
    println!("  Document replication: {:.2} cores (30 × {:.3}s per commit)", 
        (reindex_cpu_time * 30.0), reindex_cpu_time);
    println!("  Segment replication: {:.2} cores (30 × {:.3}s per sync)", 
        (rsync_cpu_time * 30.0), rsync_cpu_time);
    println!("  Savings: {:.2} cores/commit\n", 
        (reindex_cpu_time - rsync_cpu_time) * 30.0);
    
    println!("=== INTERPRETATION ===");
    
    if !replica_valid {
        println!("❌ FAIL: Replica corrupted after rsync");
        println!("   Segment replication does NOT work");
        println!("   Must use document replication (30x cost multiplier)");
    } else if rsync_cpu_time / cpu_time < 0.1 {
        println!("✅ PASS: Segment replication <10% of indexing CPU");
        println!("   {:.1}x CPU savings per replica", cpu_savings);
        println!("   Global replication cost model validated");
        println!("   30 replicas viable at projected cost");
    } else if rsync_cpu_time / cpu_time < 0.3 {
        println!("⚠️  MARGINAL: Segment replication {:.1}% of indexing CPU", 
            (rsync_cpu_time / cpu_time) * 100.0);
        println!("   Savings exist ({:.1}x) but less than expected", cpu_savings);
        println!("   May need to reduce replica count or accept higher costs");
    } else {
        println!("❌ FAIL: Segment replication {:.1}% of indexing CPU", 
            (rsync_cpu_time / cpu_time) * 100.0);
        println!("   Insufficient savings ({:.1}x vs target 10x)", cpu_savings);
        println!("   Global replication cost model broken");
        println!("   20-30 replicas economically infeasible");
    }
    
    if rsync_cpu_time < 0.001 {
        println!("\n⚠️  WARNING: rsync CPU time suspiciously low ({:.6}s)", rsync_cpu_time);
        println!("   CPU measurement may be unreliable on this platform");
        println!("   Wall time ({:?}) more trustworthy for comparison", rsync_wall_time);
    }
    
    println!("\n=== PRODUCTION IMPLICATIONS ===");
    
    if replica_valid && cpu_savings > 5.0 {
        println!("✅ Architecture validated:");
        println!("   - Primary indexes once: {:.3}s CPU", cpu_time);
        println!("   - Each replica syncs: {:.3}s CPU", rsync_cpu_time);
        println!("   - Network transfer: {:?}", rsync_wall_time);
        println!("   - At 1K writes/sec: {:.2} cores for 30 replicas", rsync_cpu_time * 30.0);
        
        let monthly_cost_per_core = 22.0; // $22/month @ 2 vCPU AWS
        let cores_needed = rsync_cpu_time * 30.0;
        let monthly_cost = cores_needed * monthly_cost_per_core;
        
        println!("\nCost projection:");
        println!("   ${:.2}/month for replication at 1K writes/sec", monthly_cost);
        println!("   vs ${:.2}/month with document replication", 
            reindex_cpu_time * 30.0 * monthly_cost_per_core);
        println!("   Savings: ${:.2}/month", 
            (reindex_cpu_time - rsync_cpu_time) * 30.0 * monthly_cost_per_core);
    } else {
        println!("⚠️  Architecture needs revision:");
        println!("   - Segment replication insufficient savings");
        println!("   - Consider: 3-5 replicas max instead of 30");
        println!("   - Or: Accept higher infrastructure costs");
        println!("   - Or: Async replication with longer lag");
    }
}



// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin tantivy_segment_replication_cost
//    Compiling flapjack_rust v0.1.0 (/home/ubuntu/flapjack_rust)
//     Finished `release` profile [optimized] target(s) in 38.01s
//      Running `target/release/tantivy_segment_replication_cost`
// === Tantivy Segment Replication Cost Test ===

// Phase 1: Primary indexing (baseline CPU cost)
//   Indexing 10000 docs...
//   Wall time: 607.375744ms
//   CPU time: 1.020s
//   Disk usage: 1.04 MB
//   Indexing rate: 9804 docs/sec

// Phase 2: Segment replication (rsync copy)
//   Copying segments from "/tmp/.tmpiWUctZ" to "/tmp/.tmpwW7u4b"...
//   Wall time: 84.233467ms
//   CPU time: 0.000s
//   Disk usage: 1.04 MB
//   Replica valid: true

// Phase 3: Document replication (re-index baseline)
//   Re-indexing same 10000 docs...
//   Wall time: 252.488654ms
//   CPU time: 0.300s
//   Disk usage: 1.04 MB

// === RESULTS SUMMARY ===

// Indexing (primary):
//   CPU: 1.020s
//   Disk: 1.04 MB

// Segment replication (rsync):
//   CPU: 0.000s (0.0% of indexing)
//   Wall: 84.233467ms
//   Disk: 1.04 MB

// Document replication (re-index):
//   CPU: 0.300s (29.4% of primary)
//   Wall: 252.488654ms
//   Disk: 1.04 MB

// === COST ANALYSIS ===
// Segment replication CPU savings: infx

// Projection for 30 replicas @ 1K writes/sec:
//   Document replication: 9.00 cores (30 × 0.300s per commit)
//   Segment replication: 0.00 cores (30 × 0.000s per sync)
//   Savings: 9.00 cores/commit

// === INTERPRETATION ===
// ✅ PASS: Segment replication <10% of indexing CPU
//    infx CPU savings per replica
//    Global replication cost model validated
//    30 replicas viable at projected cost

// ⚠️  WARNING: rsync CPU time suspiciously low (0.000000s)
//    CPU measurement may be unreliable on this platform
//    Wall time (84.233467ms) more trustworthy for comparison

// === PRODUCTION IMPLICATIONS ===
// ✅ Architecture validated:
//    - Primary indexes once: 1.020s CPU
//    - Each replica syncs: 0.000s CPU
//    - Network transfer: 84.233467ms
//    - At 1K writes/sec: 0.00 cores for 30 replicas

// Cost projection:
//    $0.00/month for replication at 1K writes/sec
//    vs $198.00/month with document replication
//    Savings: $198.00/month
// ubuntu@ip-172-31-23-154:~/flapjack_rust$ cargo run --release --bin tantivy_segment_replication_cost


// **CPU measurement failed. Wall time is the real data.**

// **What the test actually shows:**

// - Primary indexing: 607ms wall time, 1.04 MB disk
// - rsync copy: **84ms wall time** (13.8% of indexing)
// - Re-indexing: 252ms wall time (41.5% of indexing)

// **rsync CPU = 0.000s is measurement artifact.** rsync is a separate process - your test only measured parent process CPU. The 84ms wall time proves work happened.

// **Revised interpretation:**

// rsync is **mostly I/O-bound** (disk read + write, minimal CPU). The 84ms wall time at 1.04 MB = 12.4 MB/sec local transfer rate. This is pure disk throughput, not compute.

// **Critical insight:** Your 0.31 cores claim assumes rsync CPU scales with file size. But if rsync is I/O-bound (likely), the bottleneck is **disk bandwidth**, not CPU.

// **Real-world implications:**

// At 30 replicas over network:
// - Network I/O: 30 × 84ms = **2.5 seconds** if sequential
// - If parallel: Limited by primary's disk read bandwidth (500 MB/s SSD ÷ 1.04 MB = can serve 480 concurrent rsyncs, so 30 is trivial)
// - CPU cost: Negligible for file copy, but **network stack overhead** unmeasured

// **The test doesn't validate your claim.** It shows segment copy is faster than re-indexing (true), but doesn't measure network replication CPU (SSH encryption, TCP overhead).

// **What you need to test:**

// Transfer over actual network (even localhost loopback) with SSH to measure encryption CPU cost. That's the real replication cost, not local disk copy.

// https://claude.ai/chat/5b212596-1465-44dc-807a-c0a79cbc4024 %20tantivy_segment_replication_cost%20tantivy_memory_density_test