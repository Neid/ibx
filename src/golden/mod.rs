//! Golden codec tests (ibx#486): frames recorded between the reference
//! gateway and the servers, replayed through ibx, and compared with what the
//! reference did.
//!
//! - Decode: the recorded server frames go through the engine's links and
//!   the API client; ibx's callbacks are compared with the callbacks the
//!   reference's API client received (type, value, size, attributes, count,
//!   order).
//! - Encode: the recorded API requests are made again; the messages ibx
//!   writes are compared with the reference's, after the session fields are
//!   normalised (`test_support::normalise`).
//!
//! The fixtures are under tests/fixtures/gw1040/codec/ (`codec/1`, made by
//! scripts/codec_fixtures.py), read and replayed by
//! `test_support::scenario`.

mod account;
mod hmds;
mod l1;
mod orders;
mod replay;

use crate::test_support::scenario::{load_codec, load_path, Scenario};

/// A codec fixture by name: the slice of the reference 10.51's recording
/// (tests/fixtures/gw1051/codec/, ibx#604), or of 10.40's when 10.51 has
/// none of that name yet.
fn load(name: &str) -> Scenario {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gw1051/codec").join(format!("{name}.jsonl"));
    if path.exists() { load_path(&path) } else { load_codec(name) }
}
