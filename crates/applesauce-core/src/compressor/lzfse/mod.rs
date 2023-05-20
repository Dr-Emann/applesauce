#[cfg(feature = "system-lzfse")]
#[path = "system.rs"]
mod imp;

// This module is only enabled if either lzfse feature is enabled, and system-lzfse takes precedence
#[cfg(not(feature = "system-lzfse"))]
#[path = "external.rs"]
mod imp;

use crate::compressor::lz;
pub use imp::Impl;

pub type Lzfse = lz::Lz<Impl>;

#[test]
fn round_trip() {
    let mut compressor = Lzfse::new();
    super::tests::compressor_round_trip(&mut compressor);
}
