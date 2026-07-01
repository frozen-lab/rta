//! Benchmarks for `read` latency
//! Run using: `taskset -c 2,3,4 cargo bench --bench read`

use hdrhistogram::Histogram;
use rta::{RTA, Rta, RtaCfg};
use std::{hint, sync, thread, time};

const MOD_ID: u8 = 0x00;
const THREADS: usize = 4;
const OPS: usize = 0x100_000;
const COPIES_ON_DISK: usize = 0x100;
const WARMUP_OPS: usize = OPS >> 10;
const OPS_PER_THREAD: usize = OPS / THREADS;

#[repr(C)]
#[repr(align(8))]
#[derive(Debug, Default, Clone, Copy, RTA, PartialEq)]
struct Type([u64; 4]);

#[derive(Debug)]
struct BenchResult {
    hist: Histogram<u64>,
}

#[inline]
fn prep_init() -> (std::path::PathBuf, RtaCfg) {
    let path =
        tempfile::NamedTempFile::new().expect("create temp file").into_temp_path().to_path_buf();
    let cfg = RtaCfg { module_id: MOD_ID, path: path.clone(), copies_on_disk: COPIES_ON_DISK };

    (path, cfg)
}

#[inline(always)]
fn record_bench(rta: &Rta<Type>, ops: usize) -> BenchResult {
    let mut hist = Histogram::<u64>::new(3).expect("new histogram");
    for _ in 0..ops {
        let start = time::Instant::now();
        let _ = hint::black_box(unsafe { rta.read() });
        hist.record(start.elapsed().as_nanos() as u64).expect("record latency");
    }

    BenchResult { hist }
}

fn single_tx_read_latency() -> BenchResult {
    let (_path, cfg) = prep_init();
    let rta = Rta::<Type>::new(cfg).expect("new Rta");

    unsafe {
        rta.write(|t| t.0 = [1; 4]).expect("seed write").wait().expect("wait");
    }

    // warmup
    for _ in 0..WARMUP_OPS {
        let _ = unsafe { rta.read() };
    }

    record_bench(&rta, OPS)
}

fn multi_tx_read_latency() -> BenchResult {
    let (_path, cfg) = prep_init();
    let rta = sync::Arc::new(Rta::<Type>::new(cfg).expect("new Rta"));

    unsafe {
        rta.write(|t| t.0 = [1; 4]).expect("seed write").wait().expect("wait");
    }

    let barrier = sync::Arc::new(sync::Barrier::new(THREADS));

    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let rta = sync::Arc::clone(&rta);
        let barrier = sync::Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            // warmup
            for _ in 0..WARMUP_OPS {
                let _ = unsafe { rta.read() };
            }

            barrier.wait();
            let result = record_bench(&rta, OPS_PER_THREAD);

            barrier.wait();
            result
        }));
    }

    let mut hist = Histogram::<u64>::new(3).expect("new histogram");
    for handle in handles {
        let result = handle.join().expect("worker should join");
        hist.add(&result.hist).expect("merge histogram");
    }

    BenchResult { hist }
}

fn print_results(single: &BenchResult, multi: &BenchResult) {
    println!();
    println!("| Metric  | Single TX (µs) | Multi TX (µs) |");
    println!("|:--------|:---------------|:--------------|");
    println!(
        "| P50     | {:>14.4} | {:>13.4} |",
        single.hist.value_at_quantile(0.50) as f64 / 1000.0,
        multi.hist.value_at_quantile(0.50) as f64 / 1000.0,
    );
    println!(
        "| P90     | {:>14.4} | {:>13.4} |",
        single.hist.value_at_quantile(0.90) as f64 / 1000.0,
        multi.hist.value_at_quantile(0.90) as f64 / 1000.0,
    );
    println!(
        "| P99     | {:>14.4} | {:>13.4} |",
        single.hist.value_at_quantile(0.99) as f64 / 1000.0,
        multi.hist.value_at_quantile(0.99) as f64 / 1000.0,
    );
    println!(
        "| MEAN    | {:>14.4} | {:>13.4} |",
        single.hist.mean() as f64 / 1000.0,
        multi.hist.mean() as f64 / 1000.0,
    );
    println!(
        "| MAX     | {:>14.4} | {:>13.4} |",
        single.hist.max() as f64 / 1000.0,
        multi.hist.max() as f64 / 1000.0,
    );
    println!();
}

fn main() {
    let single = single_tx_read_latency();
    let multi = multi_tx_read_latency();

    print_results(&single, &multi);
}
