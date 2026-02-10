extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput};

const FNV_PRIME: u64 = 0x100000001b3;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// custom impl of fnv1a hasher
fn hasher(mut hash: u64, bytes: &[u8]) -> u64 {
    if hash == 0 {
        hash = FNV_OFFSET;
    }
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[proc_macro_derive(RTA)]
pub fn derive_rta(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;

    let mut hash = hasher(0, ident.to_string().as_bytes());

    let fields = match input.data {
        Data::Struct(s) => s.fields,
        _ => panic!("RTA can only be derived for a *struct*"),
    };

    for field in fields {
        let ty = field.ty;
        let ty_str = quote!(#ty).to_string();
        hash = hasher(hash, ty_str.as_bytes());
    }

    let expanded = quote! {
        unsafe impl rta::RTA for #ident {
            const SIZE: usize = core::mem::size_of::<Self>();
            const LAYOUT_HASH: u64 = #hash;
        }
    };

    TokenStream::from(expanded)
}
