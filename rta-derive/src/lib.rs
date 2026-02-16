#![doc = include_str!("../README.md")]

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput};

/// Derives the `rta::RTA` trait for a struct `T`
///
/// ## Important
///
/// - type `T` must use `repr(C)`
/// - should not implement Drop (to avoid undefined behaviour)
///
/// ## Why?
///
/// `#[derive(RTA)]` implementation, computes a compile time `HASH`, which is used as unique
/// and deterministic id for a given type `T`
///
/// This is to track any changes in the implementation of type `T`
#[proc_macro_derive(RTA)]
pub fn derive_rta(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let mut hash = hasher::hash(0, ident.to_string().as_bytes());

    let fields = match input.data {
        Data::Struct(s) => s.fields,
        _ => {
            return syn::Error::new_spanned(ident, "RTA can only be derived for a struct")
                .to_compile_error()
                .into();
        }
    };

    for field in fields {
        let ty = field.ty;
        let ty_str = quote!(#ty).to_string();
        hash = hasher::hash(hash, ty_str.as_bytes());
    }

    let expanded = quote! {
        unsafe impl rta::RTA for #ident {
            const HASH: u64 = #hash;
        }
    };

    TokenStream::from(expanded)
}

mod hasher {
    const FNV_PRIME: u64 = 0x100000001b3;
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;

    /// custom impl of `fnv1a` hasher
    pub const fn hash(mut hash: u64, bytes: &[u8]) -> u64 {
        if hash == 0 {
            hash = FNV_OFFSET;
        }

        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
            i += 1;
        }

        hash
    }

    #[cfg(test)]
    mod tests {
        use super::hash;

        #[test]
        fn same_input_same_hash() {
            let h1 = hash(0, b"u64");
            let h2 = hash(0, b"u64");
            assert_eq!(h1, h2);
        }

        #[test]
        fn different_inputs_different_hashes() {
            let h1 = hash(0, b"u64");
            let h2 = hash(0, b"u32");
            assert_ne!(h1, h2);
        }

        #[test]
        fn order_sensitive_hashing() {
            let h1 = hash(0, b"u64u32");
            let h2 = hash(0, b"u32u64");
            assert_ne!(h1, h2);
        }

        #[test]
        fn hasher_incremental_equals_one_shot() {
            let mut h = hash(0, b"u64");
            h = hash(h, b"u32");

            let one_shot = hash(0, b"u64u32");

            assert_eq!(h, one_shot);
        }

        #[test]
        fn hasher_empty_is_offset_basis() {
            let h = hash(0, b"");
            assert_eq!(h, 0xcbf29ce484222325);
        }
    }
}
