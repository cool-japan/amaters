//! Cryptographic utilities for AmateRS.

pub mod constant_time;
pub use constant_time::{constant_time_eq, constant_time_select, constant_time_select_slice};
