[![Latest Version](https://img.shields.io/crates/v/rta.svg)](https://crates.io/crates/rta)
[![License](https://img.shields.io/github/license/frozen-lab/rta?logo=open-source-initiative&logoColor=white)](https://github.com/frozen-lab/rta/blob/master/LICENSE)
[![Tests](https://github.com/frozen-lab/rta/actions/workflows/tests.yaml/badge.svg)](https://github.com/frozen-lab/rta/actions/workflows/tests.yaml)

# Ṛta (ऋत)

Ṛta (ऋत) is a minimal metadata store for durable system state.

## Usage

Add following to your `Cargo.toml`,

```toml
[dependencies]
rta = { version = "0.0.3" }
```

> [!NOTE]
> Current `rta` requires Rust 1.86 or later.

## The type `T`

T must satisfy following layout and safety constraints:

- Implements `RTA`
- Uses `#[repr(C)]`
- 8 bytes alignment
- Does not implements `Drop`
- Size must be >0 and multiple of 8
- Implements `Default + Clone + Send + Sync + 'static`

## Requirements for type `T`

The type `T` must not,

- Have any non-deterministic feilds
- Have interior pointers or self-references
- Have changes made to the layout after `Rta::init()`

## Write

The `Rta::write` is designed as a lightweight fire and forget metadata update premitive. The
call itself does not wait for the filesystem durability, allowing sub-mirco second write latency.

Apps requireing stronger durability can explicitly wait or block on the returned
`frozen_core::ack::AckTicket` using either the asynchronous `await` interface or the internal
blocking `wait()` api.

Every write guarantees,

* Serialized object updates
* Crash-safe durability using multiple on-disk copies
* Automatic recovery from torn or interrupted writes by selecting the newest valid version during
  initialization
  
Observed measurements for latency (both single and multi-threaded) on `1,048,576` operations,

| Metric | Single TX (µs) | Multi TX (µs) |
|:------ |:---------------|:--------------|
| P50    | 0.1830         | 0.4590        |
| P90    | 0.2750         | 0.7330        |
| P99    | 1216.5110      | 1.0090        |
| Mean   | 38.0899        | 8.7255        |
| Max    | 28606.4630     | 21790.7190    |

Observed measurements for latency (both single and multi-threaded) on `4,096` operations, where
every write waits for filesystem durability,

| Metric | Single TX (ms) | Multi TX (ms) |
|:------ |:---------------|:--------------|
| P50    | 1.1899         | 1.2820        |
| P90    | 1.2902         | 2.8078        |
| P99    | 1.5165         | 5.2634        |
| Mean   | 1.1885         | 1.7603        |
| Max    | 31.3917        | 30.1793       |

## Read

The `Rta::read` is a wait-free fast-path read which reads the most recently updated `T`
directly from the underlying memory mapping.

A successful read reflects the latest published write, but does not imply that the object has
been durably persisted to the filesystem. To guarantee durability, wait on the ticket returned
by `Rta::write`.

Observed measurements for latency (both single and multi threaded) on `1,048,576` individual ops,

| Metric  | Single TX (µs) | Multi TX (µs) |
|:--------|:---------------|:--------------|
| P50     | 0.0000         | 0.0910        |
| P90     | 0.0910         | 0.1830        |
| P99     | 0.0920         | 0.8210        |
| MEAN    | 0.0205         | 0.1362        |
| MAX     | 38.0790        | 5013.5030     |

## Example

```rs
use rta::{RTA, Rta, RtaCfg};

#[repr(C)]
#[repr(align(8))]
#[derive(Debug, Clone, Default, RTA)]
struct Config {
    value: u64,
}

let dir = tempfile::tempdir().expect("failed to create temp directory");
let rta = Rta::<Config>::new(RtaCfg {
    module_id: 0,
    copies_on_disk: 4,
    path: dir.path().join("config.rta"),
})
.expect("failed to create RTA");

let ticket = unsafe {
    rta.write(|cfg| {
        cfg.value = 0x2A;
    })
}
.expect("write failed");
ticket.wait().expect("durability failed");

let cfg = unsafe { rta.read() };
assert_eq!(cfg.value, 0x2A);
```

## Etymology

ऋत (transliterated as Ṛta) is a _vedic_ concept of cosmic order, truth, and invariance that
inspired the design of Ṛta crate.
