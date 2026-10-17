//! ibapi-compatible Rust API.
//!
//! Provides `Contract`, `Order`, `Wrapper` trait, and `EClient` — matching the
//! reference C++ API (EClientSocket / EWrapper) pattern.

pub mod client;
pub mod config;
pub mod types;
pub mod wrapper;
#[cfg(test)]
pub(crate) mod contract_details_golden;

pub use client::{EClient, EClientConfig};
pub use config::*;
pub use types::*;
pub use wrapper::Wrapper;
