# `rta-derive`

Procedural macro implementation for `#[derive(RTA)]`

## `RTA`

It computes a deterministic hash(u64) at _compile time_ for given struct `T` using,

- name of `T`
- ordered list of field types in `T`

The generated hash is intended for type identity and layout fingerprinting

> [!CAUTION]
> the `HASH` is not cryptographically secure, and not ment to be either ;)
