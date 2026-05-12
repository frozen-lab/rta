use rta::{Rta, RTA};
use std::{path::PathBuf, time::Instant};

const OPS: usize = 0x2710;

#[repr(C)]
#[derive(Default, Clone, Copy, RTA)]
struct BenchType {
    a: u64,
    b: u64,
    c: u64,
    d: u64,
}

fn percentile(sorted: &[u128], pct: f64) -> u128 {
    let idx = ((sorted.len() as f64) * pct) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn main() {
    let path = temp_path();
    let rta = Rta::<BenchType, 0>::new(&path).expect("failed to create Rta");
    let mut samples = Vec::with_capacity(OPS);

    for i in 0..OPS {
        let start = Instant::now();
        rta.write(|t| {
            t.a = i as u64;
            t.b = i as u64;
        })
        .expect("write failed");

        samples.push(start.elapsed().as_nanos());
    }

    samples.sort_unstable();
    let total: u128 = samples.iter().sum();

    let avg = total / OPS as u128;
    let p50 = percentile(&samples, 0.50);
    let p90 = percentile(&samples, 0.90);
    let p99 = percentile(&samples, 0.99);
    let p999 = percentile(&samples, 0.999);

    println!("Rta write latency benchmark");
    println!("ops      : {}", OPS);
    println!("avg(ns)  : {}", avg);
    println!("p50(ns)  : {}", p50);
    println!("p90(ns)  : {}", p90);
    println!("p99(ns)  : {}", p99);
    println!("p999(ns) : {}", p999);
}

fn temp_path() -> PathBuf {
    tempfile::NamedTempFile::new()
        .expect("failed to create tempfile")
        .into_temp_path()
        .to_path_buf()
}
