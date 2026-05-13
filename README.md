[![Latest Version](https://img.shields.io/crates/v/frozen_core.svg)](https://crates.io/crates/frozen_core)
[![License](https://img.shields.io/github/license/frozen-lab/frozen_core?logo=open-source-initiative&logoColor=white)](https://github.com/frozen-lab/frozen_core/blob/master/LICENSE)
[![Tests](https://github.com/frozen-lab/frozen_core/actions/workflows/tests.yaml/badge.svg)](https://github.com/frozen-lab/frozen_core/actions/workflows/tests.yaml)

# Ṛta (ऋत)

Ṛta (ऋत) is a minimal metadata store for durable system state.

## The type `T`

T must satisfy following layout and safety constraints:

- Implements `RTA`
- Uses `#[repr(C)]`
- 8 bytes alignment
- Does not implements `Drop`
- Size must be >0 and multiple of 8
- Implements `Default + Clone + Send + Sync + 'static`

#### Limitations of `T`

- No non-deterministic feilds
- No interior pointers or self-references
- Changes made in struct layout break compatibility

## Example

```rs
use rta::{Rta, RTA};

#[repr(C)]
#[derive(Default, Clone, Copy, RTA)]
struct TestType {
    a: u64,
    b: u64,
}

const MOD_ID: u8 = 0;

let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();

rta.write(|t| {
  t.a = 0x20;
  t.b = 0x40;
}).unwrap();

let val = rta.read().unwrap();
assert_eq!(val.a, 0x20);
assert_eq!(val.b, 0x40);
```

## Writes

The [`Rta::write()`] operation in `Rta` is designed as a lightweight _fire-and-forget_ metadata update primitive.
This call itself does **not** wait for disk synchronization on the fast path, allowing extremely low write latency
while still preserving crash-safe durability semantics by handing off durability responsibility to a background
thread.

#### Guarantees

Following guarantees are provided,

- serialized metadata updates
- crash-safe durability via multiple on-disk copies
- automatic recovery from torn writes using latest valid version

#### Benchmarks

| Metric  | Latency |
|:--------|:--------|
| Average | 83 ns   |
| P50     | 101 ns  |
| P90     | 102 ns  |
| P99     | 102 ns  |
| P999    | 102 ns  |

#### Notes

- Most writes complete in ~100ns on the uncontended fast path.
- The benchmark primarily measures,
  - lock acquisition
  - in-memory mutation
  - dirty state propagation
- Disk durability is handled asynchronously by a dedicated background thread.
- Tail latency may increase if writes overlap with active durability synchronization.

## Reads

The [`Rta::read()`] operation returns the latest available in-memory metadata state.

The read path is optimistic and does not provide durability guarantees at read time. Instead, reads are served
directly from the synchronized in-memory cache, avoiding any disk IO overhead.

#### Guarantees

Following guarantees are provided,

- lock-safe concurrent access
- latest available in-memory metadata view
- no torn or partially visible updates
- parallel read operations

#### Notes

- Reads are performed entirely from the in-memory cache.
- No mmap scan or disk synchronization occurs during the read path.
- Read latency generally remains stable even during background durability synchronization.

## Concurrency Model

| Operation        | Parallelism | Blocks Reads | Blocks Writes |
|:-----------------|-------------|--------------|---------------|
| **Read**         | Yes         | No           | No            |
| **Write**        | Limited     | No           | Sometimes     |

## Etymology

ऋत (transliterated as Ṛta) is a _Vedic_ concept of cosmic order, truth, and invariance that inspired the design
of Ṛta crate.
