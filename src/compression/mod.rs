//! Decompress and Compress various Nintendo compression formats.
//!
//! Compression supported:
//! - [Yaz0][`yaz0`]

#[cfg(feature = "yaz0")]
pub mod yaz0;

pub use yaz0::Yaz0Reader;
