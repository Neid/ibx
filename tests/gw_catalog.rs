//! Layer A of the proof that ibx behaves like the gateway (ibx#485): table
//! tests whose expected values all come from the catalogs extracted from
//! the gateway's JAR, 10.51 where its table is made and 10.40 for the
//! others (`tests/fixtures/<version>/catalog/`, ibx#604). Offline, through
//! the real ibx code paths, on the in-memory transport.

#[path = "gw_catalog/catalog.rs"]
mod catalog;
#[path = "gw_catalog/harness.rs"]
mod harness;
#[path = "gw_catalog/writers.rs"]
mod writers;
#[path = "gw_catalog/errors.rs"]
mod errors;
#[path = "gw_catalog/refusals.rs"]
mod refusals;
