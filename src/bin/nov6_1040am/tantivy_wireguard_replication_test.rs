use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tantivy::schema::{Schema, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};
use tempfile::TempDir;

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

fn get_system_cpu_stats() -> (f64, f64) {
    if let Ok(contents) = fs::read_to_string("/proc/stat") {
        if let Some(cpu_line) = contents.lines().next() {
            let parts: Vec<&str> = cpu_line.split_whitespace().collect();
            if parts.len() > 4 && parts[0] == "cpu" {
                let user: u64 = parts[1].parse().unwrap_or(0);
                let nice: u64 = parts[2].parse().unwrap_or(0);
                let system: u64 = parts[3].parse().unwrap_or(0);
                let idle: u64 = parts[4].parse().unwrap_or(0);
                let iowait: u64 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
                let total = user + nice + system + idle + iowait;
                return (total as f64, idle as f64);
            }
        }
    }
    (0.0, 0.0)
}

fn rsync_over_wireguard(src: &Path, dst_ip: &str, dst_path: &str) -> Result<(std::time::Duration, f64), std::io::Error> {
    let (cpu_before, idle_before) = get_system_cpu_stats();
    let start = Instant::now();
    
    // Rsync over WireGuard (no SSH, just raw rsync daemon or via SSH over WireGuard)
    // Using SSH over WireGuard for simplicity (SSH connects to 10.0.0.x instead of public IP)
    let output = Command::new("rsync")
        .arg("-a")
        .arg("--delete")
        .arg("-e")
        .arg("ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null")
        .arg(format!("{}/", src.display()))
        .arg(format!("ubuntu@{}:{}/", dst_ip, dst_path))
        .output()?;
    
    let elapsed = start.elapsed();
    
    std::thread::sleep(std::time::Duration::from_millis(100));
    let (cpu_after, idle_after) = get_system_cpu_stats();
    
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("rsync failed: {}", String::from_utf8_lossy(&output.stderr))
        ));
    }
    
    let total_cpu_used = (cpu_after - cpu_before) - (idle_after - idle_before);
    let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let cpu_seconds = total_cpu_used / clock_ticks;
    
    Ok((elapsed, cpu_seconds))
}

fn main() {
    println!("=== Tantivy WireGuard Replication Test ===\n");
    
    // Configuration
    let remote_ip = "10.0.0.2"; // WireGuard IP of remote instance
    let remote_path = "/tmp/wireguard_replica";
    
    // Verify WireGuard connectivity
    println!("Testing WireGuard connectivity to {}...", remote_ip);
    let ping = Command::new("ping")
        .args(&["-c", "3", remote_ip])
        .output();
    
    if ping.is_err() || !ping.unwrap().status.success() {
        eprintln!("ERROR: Cannot reach {} via WireGuard", remote_ip);
        eprintln!("Verify: sudo wg show");
        eprintln!("Expected: peer with endpoint and latest handshake");
        std::process::exit(1);
    }
    println!("✓ WireGuard tunnel active\n");
    
    let doc_count = 10_000;
    
    println!("Phase 1: Indexing {} docs (baseline CPU)...", doc_count);
    let mut primary = TenantIndex::new();
    let primary_path = primary.path().to_path_buf();
    
    let start = Instant::now();
    primary.add_documents(doc_count);
    primary.commit().unwrap();
    
    let indexing_wall = start.elapsed();
    let primary_disk = primary.get_disk_usage();
    
    println!("  Wall time: {:?}", indexing_wall);
    println!("  Disk: {:.2} MB\n", primary_disk as f64 / 1_000_000.0);
    
    // Prepare remote directory
    println!("Phase 2: Preparing remote directory...");
    Command::new("ssh")
        .args(&[
            "-o", "StrictHostKeyChecking=no",
            &format!("ubuntu@{}", remote_ip),
            &format!("mkdir -p {}", remote_path)
        ])
        .output()
        .expect("Failed to create remote directory");
    
    println!("Phase 3: WireGuard replication (rsync over tunnel)...");
    
    let (wg_wall, wg_cpu) = match rsync_over_wireguard(&primary_path, remote_ip, remote_path) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("\nERROR: WireGuard rsync failed: {}", e);
            eprintln!("Verify SSH keys are set up for ubuntu@{}", remote_ip);
            std::process::exit(1);
        }
    };
    
    println!("  Wall time: {:?}", wg_wall);
    println!("  CPU time: {:.3}s (system-wide)\n", wg_cpu);
    
    // Verify replica
    let verify = Command::new("ssh")
        .args(&[
            &format!("ubuntu@{}", remote_ip),
            &format!("du -sb {}", remote_path)
        ])
        .output();
    
    let replica_valid = if let Ok(output) = verify {
        if output.status.success() {
            let size_str = String::from_utf8_lossy(&output.stdout);
            let size: u64 = size_str.split_whitespace().next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            println!("  Replica disk: {:.2} MB", size as f64 / 1_000_000.0);
            size > 0
        } else {
            false
        }
    } else {
        false
    };
    
    println!("  Replica valid: {}\n", replica_valid);
    
    drop(primary);
    
    println!("=== RESULTS ===\n");
    
    println!("WireGuard replication:");
    println!("  CPU: {:.3}s", wg_cpu);
    println!("  Wall: {:?}", wg_wall);
    println!("  Bandwidth: {:.2} MB/s", (primary_disk as f64 / 1_000_000.0) / wg_wall.as_secs_f64());
    
    let cpu_per_replica = wg_cpu;
    println!("\n=== COST ANALYSIS ===");
    
    let replicas = 30;
    let writes_per_sec = 10.0; // 1K writes/sec, 100 docs/batch
    
    println!("Projection for {} replicas @ 1K writes/sec:", replicas);
    println!("  CPU per commit: {:.3}s × {} = {:.2}s", cpu_per_replica, replicas, cpu_per_replica * replicas as f64);
    println!("  Sustained load: {:.2} cores", cpu_per_replica * replicas as f64 * writes_per_sec);
    
    let cores_needed = cpu_per_replica * replicas as f64 * writes_per_sec;
    let monthly_cost = cores_needed * 22.0;
    
    println!("\nInfrastructure cost:");
    println!("  {:.2} cores @ $22/core/month", cores_needed);
    println!("  ${:.2}/month\n", monthly_cost);
    
    println!("=== INTERPRETATION ===");
    
    if !replica_valid {
        println!("❌ FAIL: Replica corrupted or inaccessible");
    } else if cores_needed < 1.0 {
        println!("✅ VALIDATED: 30 replicas viable");
        println!("   {:.2} cores at 1K writes/sec", cores_needed);
        println!("   Original 0.31 cores claim: {}", 
            if cores_needed < 0.5 { "CONFIRMED" } else { "Close enough" });
    } else if cores_needed < 3.0 {
        println!("⚠️  MARGINAL: {:.2} cores for 30 replicas", cores_needed);
        println!("   Viable but adjust cost model");
        println!("   Consider 10-15 replicas instead");
    } else {
        println!("❌ FAIL: {:.2} cores for 30 replicas", cores_needed);
        println!("   Too expensive - regional primaries required");
    }
    
    println!("\n=== COMPARISON ===");
    println!("SSH test result: 72.00 cores (232x over budget)");
    println!("WireGuard result: {:.2} cores ({:.1}x {})", 
        cores_needed,
        cores_needed / 0.31,
        if cores_needed < 1.0 { "UNDER budget" } else { "over budget" }
    );
    println!("Improvement: {:.1}x faster", 72.0 / cores_needed);
}



//      Running `target/release/tantivy_wireguard_replication_test`
// === Tantivy WireGuard Replication Test ===

// Testing WireGuard connectivity to 10.0.0.2...
// ✓ WireGuard tunnel active

// Phase 1: Indexing 10000 docs (baseline CPU)...
//   Wall time: 152.944609ms
//   Disk: 1.04 MB

// Phase 2: Preparing remote directory...
// Phase 3: WireGuard replication (rsync over tunnel)...
//   Wall time: 1.775092959s
//   CPU time: 0.150s (system-wide)

//   Replica disk: 1.04 MB
//   Replica valid: true

// === RESULTS ===

// WireGuard replication:
//   CPU: 0.150s
//   Wall: 1.775092959s
//   Bandwidth: 0.58 MB/s

// === COST ANALYSIS ===
// Projection for 30 replicas @ 1K writes/sec:
//   CPU per commit: 0.150s × 30 = 4.50s
//   Sustained load: 45.00 cores

// Infrastructure cost:
//   45.00 cores @ $22/core/month
//   $990.00/month

// === INTERPRETATION ===
// ❌ FAIL: 45.00 cores for 30 replicas
//    Too expensive - regional primaries required

// === COMPARISON ===
// SSH test result: 72.00 cores (232x over budget)
// WireGuard result: 45.00 cores (145.2x over budget)
// Improvement: 1.6x faster
// ubuntu@ip-172-31-23-154:~/flapjack_rust$