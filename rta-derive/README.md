# `rta-derive`

Procedural macro implementation for `#[derive(RTA)]`

## `#[derive(RTA)]`

It generates an implementation of the `rta::RTA` trait for a struct `T`.

At compile time it computes:

- `SIZE`: `core::mem::size_of::<T>()`
- `HASH`: a deterministic `u64` layout fingerprint

## Hash (`[T::HASH]`)

The `[T::HASH]` is computed from:

- the name of `T`
- the ordered list of _field names_
- the ordered list of _field types_

`[T::HASH]` is used to detcect type identity & schema changes for the type `T`

> [!CAUTION]
> The [`T::HASH`] is not cryptographically secure, and is not intended to be ;).
> It is only used as a deterministic layout fingerprint.

## Compile-time Checks

`RTA` structs are intended to be,

- read without serialization
- checksummed using raw byte access
- stored directly in memory-mapped files

These features requires `RTA` type to be,

- padding-free
- deterministic
- byte-stable across compilations

To achieve these, the `#[derive(RTA)]` macro performs following compile-time checks:

- `T` uses `#[repr(C)]`
- `T` is not zero-sized
- `T` contains no padding
- `size_of::<T>()` is multiple of 8 bytes

If any of these requirements are violated the compilation fails ;-)

> [!NOTE]
> We require `size_of::<T>()` to be multiple of 8 bytes, this is done for better performance while calculating
> Cyclic Redundency Check (CRC)

## Safety Requirements

Not all constraints can be enforced by [compile time checks](#compile-time-checks), these must be followed by users:

- `T` must not implement `Drop`
- fields should be plain data (POD-like)
- `T` must be `Sized`, and implement `Default`

Violating these rules may cause undefined behavior when the struct is stored directly in memory-mapped storage.
