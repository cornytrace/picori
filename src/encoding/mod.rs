//! Encode and Decode various string encodings used by Nintendo for development.
//!
//! Formats supported:
//! - [ASCII][`Ascii`]
//! - [JIS X 0201][`JisX0201`]
//! - [Shift JIS (1997)][`ShiftJis1997`]
//! - [Shift JIS (2004)][`ShiftJis2004`]

#[cfg(feature = "ascii")]
mod ascii;
#[cfg(feature = "jis_x_0201")]
mod jis_x_0201;
#[cfg(feature = "shift_jis_1997")]
mod shift_jis_1997;
#[cfg(feature = "shift_jis_2004")]
mod shift_jis_2004;

#[cfg(feature = "ascii")]
pub use ascii::{Ascii, IteratorExt as AsciiIteratorExt};
#[cfg(feature = "jis_x_0201")]
pub use jis_x_0201::{IteratorExt as JisX0201IteratorExt, JisX0201};
#[cfg(feature = "shift_jis_1997")]
pub use shift_jis_1997::{IteratorExt as ShiftJis1997IteratorExt, ShiftJis1997};
#[cfg(feature = "shift_jis_2004")]
pub use shift_jis_2004::{IteratorExt as ShiftJis2004IteratorExt, ShiftJis2004};