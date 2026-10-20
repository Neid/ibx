//! The frame filters of the scenario replays (ibx#515), shared by the Rust
//! tests and the Python replays: frames that depend on what the reference
//! had before a recording, left out on both sides.

use crate::test_support::Fields;

/// An exchange map entry: the reference had the map from earlier in its
/// session.
pub fn exchange_map(f: &Fields) -> bool {
    f.iter().any(|(t, v)| *t == 264 && v == "626")
}

/// An exchange map entry, or a frame on AAPL, whose quote and trade the
/// reference kept from the earlier scenarios of its process.
pub fn exchange_map_or_aapl(f: &Fields) -> bool {
    exchange_map(f) || f.iter().any(|(t, v)| *t == 6008 && v == "265598")
}

/// A frame of the "Core" model, which the reference asks at its logon.
pub fn core_model(f: &Fields) -> bool {
    f.iter().any(|(t, v)| *t == 6700 && v == "Core")
}

/// The cancels of the dividends' entries: the reference split the ones of
/// one frame into two messages, a timing of its threads.
pub fn dividends_cancel(f: &Fields) -> bool {
    f.iter().any(|(t, v)| *t == 263 && v == "2") && f.iter().filter(|(t, _)| *t == 264).all(|(_, v)| v == "456")
}

/// Without the fundamentals entry and the exchange map entry (the
/// reference had both from earlier in its session): their groups of
/// fields, and as many less in the entry count.
pub fn without_fundamentals_and_map(f: &mut Fields) {
    while let Some(at) = f.iter().position(|(t, v)| *t == 264 && (v == "258" || v == "626")) {
        let start = f[..at].iter().rposition(|(t, _)| *t == 262).unwrap_or(at);
        let end = f[at + 1..].iter().position(|(t, _)| *t == 262).map_or(f.len(), |n| at + 1 + n);
        f.drain(start..end);
        if let Some(count) = f.iter_mut().find(|(t, _)| *t == 146) {
            count.1 = (count.1.parse::<usize>().unwrap_or(1) - 1).to_string();
        }
    }
}

/// A frame filter by its name, for the Python replays.
pub fn skip_frame_named(name: &str) -> Option<fn(&Fields) -> bool> {
    Some(match name {
        "exchange_map" => exchange_map,
        "exchange_map_or_aapl" => exchange_map_or_aapl,
        "core_model" => core_model,
        "dividends_cancel" => dividends_cancel,
        _ => return None,
    })
}

/// A frame mask by its name, for the Python replays.
pub fn frame_mask_named(name: &str) -> Option<fn(&mut Fields)> {
    Some(match name {
        "without_fundamentals_and_map" => without_fundamentals_and_map,
        _ => return None,
    })
}
