# Ṛta (ऋत)

Ṛta (ऋत) is a minimal metadata store for durable system state.

## Requirements of `T`

T must satisfy following layout and safety constraints:

- Implements `RTA`
- Uses `#[repr(C)]`
- 8 bytes alignment
- Does not implements `Drop`
- Size must be >0 and multiple of 8
- Implements `Default + Clone + Send + Sync + 'static`

## Limitations of `T`

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

## Write Operations

The [`Rta::write`] call is _fire-and-forget_. It mutates in-mem state and triggers durable flush via a background
thread, providing close to none IO overhead for the caller.

Multiple write call, when made rightly one-after-another, may coalesce into fewer disk writes.

## Read Operations

The [`Rta::read`] returns the latest in-mem state. By default the read is optimistic and does not guarantee durability
at read time.

## Concurrency Model

| Operation        | Parallelism | Blocks Reads | Blocks Writes |
|:-----------------|-------------|--------------|---------------|
| **Read**         | Yes         | No           | No            |
| **Write**        | Limited     | No           | Sometimes     |

## Etymology

ऋत (transliterated as Ṛta) is a _Vedic_ concept of cosmic order, truth, and invariance that inspired the design
of Ṛta crate.
