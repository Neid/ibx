//! Top of book (acknowledgements, definitions and tick messages):
//! decode against the reference's callbacks.

use super::load;
use super::replay::replay_market_data;
use crate::test_support::scenario::assert_same_callbacks;

const MD_CALLBACKS: &[&str] = &[
    "tickPrice", "tickSize", "tickString", "tickGeneric", "marketDataType", "tickReqParams", "tickSnapshotEnd",
];

// AAPL then SPY before the open (19/10/2026 on 10.51 at level 225,
// b1_441_smart_components, ibx#604; first recorded 02/10/2026 on 10.40; up
// to the EUR.USD request, which goes to another farm): the acknowledgements
// give the market data type and the request parameters; the first tick message
// gives the last time, the last with its size, the volume, close, open,
// bid and ask with their sizes; no bid or ask exchange before the exchange
// map came (an empty text is not sent); the next frames give the volume,
// the ask size and the ask exchanges. The smart components requests: 321
// for an unknown code, the map once it came.
#[test]
fn aapl_and_spy_before_the_open() {
    let fx = load("l1_aapl_spy_preopen");
    let mut keep = MD_CALLBACKS.to_vec();
    keep.extend(["smartComponents", "error"]);
    let r = replay_market_data(&fx, "usfarm", &keep, Some(2742));
    assert!(r.unsent.is_empty(), "{:?}", r.unsent);
    // The farm notices of the session start are not part of the replay.
    let theirs: Vec<String> = r.theirs.into_iter().filter(|l| !l.starts_with("error|-1|")).collect();
    let ticks = theirs.iter().filter(|l| l.starts_with("tick")).count();
    assert!(ticks > 25 && theirs.len() > ticks, "{theirs:#?}");
    assert_same_callbacks(&r.ours, &theirs);
}

/// Replay a whole market data fixture on the primary farm and compare the
/// callbacks of the requests `reqs` (all when empty).
fn replay_and_compare(name: &str, until: Option<u64>, reqs: &[i64]) -> usize {
    let fx = load(name);
    let mut keep = MD_CALLBACKS.to_vec();
    keep.push("error");
    let r = replay_market_data(&fx, "usfarm", &keep, until);
    // Definition replies of lookups ibx does not make (the reference's
    // own) are left out; every farm frame goes.
    assert!(r.unsent.iter().all(|(_, conn)| conn == "CCP"), "{name}: frames not sent {:?}", r.unsent);
    let of_reqs = |l: &String| !l.starts_with("error|-1|")
        && (reqs.is_empty() || reqs.iter().any(|id| l.split('|').nth(1) == Some(&id.to_string())));
    let theirs: Vec<String> = r.theirs.into_iter().filter(of_reqs).collect();
    let ours: Vec<String> = r.ours.into_iter().filter(of_reqs).collect();
    assert_same_callbacks(&ours, &theirs);
    theirs.len()
}

#[test]
fn spy_and_qqq_in_the_session() {
    // Up to the second SPY request: the reference then held a SPY record
    // of its own with data (the orders of the scenario), which the new
    // request joined; that record's frames are not in the recording.
    assert!(replay_and_compare("l1_spy_qqq_rth", Some(6581), &[]) > 40);
}

// AAPL before the open with delayed data asked (19/10/2026 on 10.51,
// premarket_order_types, ibx#604; first recorded 28/09/2026): the paper session has real-time data, so the
// request gets type 1; the ask exchanges follow each ask size change. BMW
// and 7203 of the same scenario are on other farms: AAPL only.
#[test]
fn aapl_with_delayed_data_asked_before_the_open() {
    let n = replay_and_compare("l1_aapl_preopen_delayed", Some(2773), &[9001]);
    assert!(n > 10, "{n}");
}

// A new AAPL request 20 s after the first was cancelled (same scenario):
// the reference sends the bid and the ask with their sizes on the
// acknowledgement, before any tick message: the values the contract's record kept
// from the cancelled request (bid 340.45 x 320, ask 340.52 x 40, the last
// ones of request 9001); the first tick message then gives the trade and daily
// fields only. ibx frees the contract's quote at the cancel and sends
// everything at the first tick message. Found by this test (ibx#508); the record
// keeps its values 45 s at least (capture of 07/10/2026), how long is not
// read yet.
#[test]
fn a_new_request_gets_the_quote_kept_from_a_cancelled_one() {
    assert!(replay_and_compare("l1_aapl_preopen_delayed", None, &[9100]) > 10);
}

/// The tick callbacks of a market data fixture when the client reads
/// after each input and when it reads once after the last frame: both
/// are the reference's (ibx#446). The other callbacks come from other
/// queues, read in their own order: left out.
fn ticks_read_once_as_the_reference(name: &str, until: Option<u64>, reqs: &[i64]) -> usize {
    const TICKS: &[&str] = &["tickPrice", "tickSize", "tickString", "tickGeneric", "tickSnapshotEnd"];
    let fx = load(name);
    let of_reqs = |l: &String| reqs.is_empty() || reqs.iter().any(|id| l.split('|').nth(1) == Some(&id.to_string()));
    let mut out = Vec::new();
    for read_each in [true, false] {
        let r = super::replay::replay_market_data_read(&fx, "usfarm", TICKS, until, &[], read_each);
        let theirs: Vec<String> = r.theirs.into_iter().filter(of_reqs).collect();
        let ours: Vec<String> = r.ours.into_iter().filter(of_reqs).collect();
        assert_same_callbacks(&ours, &theirs);
        out.push(ours);
    }
    assert_eq!(out[0], out[1], "{name}: reading once changed the callbacks");
    out[0].len()
}

// ibx#446: the reference sends each farm message's ticks with that
// message's values: a client that reads once after every frame of the
// session gets the same ticks as one that reads after each frame.
#[test]
fn tick_callbacks_do_not_depend_on_when_the_client_reads() {
    assert!(ticks_read_once_as_the_reference("l1_aapl_spy_preopen", Some(2742), &[]) > 25);
    assert!(ticks_read_once_as_the_reference("l1_spy_qqq_rth", Some(6581), &[]) > 40);
    let n = ticks_read_once_as_the_reference("l1_aapl_preopen_delayed", Some(2773), &[9001]);
    assert!(n > 10, "{n}");
}

/// A 10.40 market data message in the 10.51 form: the mark 1 at the end
/// of each bid/ask and last entry (ibx#648).
fn with_entry_marks(msg: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for (i, entry) in msg.split("|262=").enumerate() {
        let marked = i > 0 && entry.split('|').any(|f| f == "264=442" || f == "264=443");
        out.push(if marked { format!("{entry}|9839=1") } else { entry.to_string() });
    }
    out.join("|262=")
}

/// The market data messages and contract lookups ibx wrote, against the
/// reference's: (ours, theirs) for the market data requests, then for the
/// security definition requests.
fn requests_of(name: &str, until: Option<u64>, skip: &[u64]) -> [(Vec<String>, Vec<String>); 2] {
    let fx = load(name);
    let r = super::replay::replay_market_data_without(&fx, "usfarm", MD_CALLBACKS, until, skip);
    let (v, c) = super::replay::request_messages(&r.session, &["V"], &["c"]);
    // The reference's messages of one message type.
    let theirs = |t: &str| r.requests_theirs.iter()
        .filter(|m| m.split('|').next().and_then(|f| f.split_once('=')).is_some_and(|(_, v)| v == t))
        .cloned().collect::<Vec<_>>();
    // Market data of contracts the reference asked on this farm only.
    // 10.51, which ibx copies, ends each bid/ask and last entry with its
    // mark (ibx#648); a recording of the reference 10.40 is read in that
    // form.
    let form_10_40 = fx.header["gateway_version"].as_str().is_some_and(|v| v.starts_with("1040"));
    let farm_v: Vec<String> = theirs("V").iter().map(|m| if form_10_40 { with_entry_marks(m) } else { m.clone() }).collect();
    let on_farm = |m: &String| farm_v.iter().any(|t| t.split('|').find(|f| f.starts_with("6008=")) == m.split('|').find(|f| f.starts_with("6008=")));
    [(v.into_iter().filter(on_farm).collect(), farm_v.clone()), (c, theirs("c"))]
}

// The requests of the top of book replays (02/10 and 28/09/2026): the
// subscribe of the bid/ask and last entries (442, 443, with the source
// Socket and the streaming-client mark), the gateway's own exchange map
// entry (626) after the acknowledgement and its cancel once the map came,
// the cancel of the request (no source field), and
// the symbol lookups: each message as the reference's, the farm ids and
// lookup ids masked. EUR.USD, BMW and 7203 go to other farms: left out.
#[test]
fn market_data_requests_as_the_reference() {
    for (name, until, skip) in [
        ("l1_aapl_spy_preopen", Some(2744), &[2742][..]),
        ("l1_spy_qqq_rth", Some(5410), &[][..]),
        ("l1_aapl_preopen_delayed", Some(2773), &[][..]),
    ] {
        for (ours, theirs) in requests_of(name, until, skip) {
            assert!(!theirs.is_empty(), "{name}");
            assert_eq!(ours, theirs, "{name}");
        }
    }
}
