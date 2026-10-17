//! The option model of an option's market data (ibx#502), replayed from
//! the reference 10.51 (paper, 12/10/2026, regular hours): an AAPL call of
//! December 2026, strike 340, given by its full description. The engine
//! gets the recorded acknowledgements and generic frames; the frames it
//! sends and the computations it queues are compared with the
//! reference's.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::bridge::SharedState;
use crate::control::generic_values::GenTick;
use crate::control::optcalc::{inputs, market, OptionComputation, OptionTerms, Right, Style, NOT_COMPUTED};
use crate::md_events::MdStep;

const OPTION: i64 = 692808239;
const AAPL: i64 = 265598;

fn frames() -> std::collections::HashMap<String, Vec<u8>> {
    let text = include_str!("../../../tests/fixtures/ibx502_frames.txt");
    text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| {
        let (name, hex) = l.split_once(' ')?;
        let bytes = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        Some((name.to_string(), bytes))
    }).collect()
}

fn terms() -> OptionTerms {
    OptionTerms {
        con_id: OPTION, sec_type: "OPT".into(), currency: "USD".into(), right: Some(Right::Call), style: Style::American,
        strike: 340.0, last_trade_date: "20261218".into(), last_trade_time: String::new(), time_zone: "US/Eastern".into(),
        under_con_id: AAPL, under_sec_type: "STK".into(),
    }
}

/// The messages the engine wrote on the farm, without the header,
/// sequence and time tags.
fn sent(server: &mut crate::protocol::connection::MemTransport) -> Vec<String> {
    use std::io::Read;
    server.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(n) = server.read(&mut chunk) {
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
    }
    let mut out = Vec::new();
    let mut rest = &buf[..];
    while let Some(len) = crate::protocol::fixcomp::fixcomp_length(rest) {
        for m in crate::protocol::fixcomp::fixcomp_decompress(&rest[..len]).unwrap() {
            let text = String::from_utf8_lossy(&m).replace('\x01', "|");
            out.push(text.split('|').filter(|f| !f.is_empty() && !f.starts_with("8=") && !f.starts_with("9=")
                && !f.starts_with("34=") && !f.starts_with("52=") && !f.starts_with("10=")).collect::<Vec<_>>().join("|"));
        }
        rest = &rest[len..];
    }
    out
}

fn ack(engine: &mut HotLoop, tag: u32, req: u32) {
    let msg = format!("8=O\x0135=Q\x01{tag},{req},0.01,0,0,ffffffff,,0,1");
    engine.farm.process_farm_message(msg.as_bytes(), &mut engine.farm_conn, &mut engine.context, &engine.shared, &None, &mut engine.hb);
}

fn generic(engine: &mut HotLoop, body: &[u8]) {
    let mut msg = b"8=O\x019=0\x0135=G\x01".to_vec();
    msg.extend_from_slice(body);
    engine.farm.process_farm_message(&msg, &mut engine.farm_conn, &mut engine.context, &engine.shared, &None, &mut engine.hb);
}

/// A top of book message: blocks of (server tag, [(tick type, value)]),
/// each value on four bytes.
fn ticks(engine: &mut HotLoop, blocks: &[(u32, &[(u64, i64)])]) {
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |v: u64, n: usize| for i in (0..n).rev() { bits.push(((v >> i) & 1) as u8) };
    for (tag, entries) in blocks {
        push(0, 1);
        push(*tag as u64, 31);
        for (k, (tick_type, value)) in entries.iter().enumerate() {
            push(*tick_type, 5);
            push((k + 1 < entries.len()) as u64, 1);
            push(3, 2);
            push((*value < 0) as u64, 1);
            push(value.unsigned_abs(), 31);
        }
    }
    let mut body = vec![(bits.len() >> 8) as u8, bits.len() as u8];
    body.resize(2 + bits.len().div_ceil(8), 0);
    for (i, b) in bits.iter().enumerate() {
        body[2 + i / 8] |= b << (7 - i % 8);
    }
    let mut msg = b"8=O\x019=0\x0135=P\x01".to_vec();
    msg.extend_from_slice(&body);
    engine.farm.process_farm_message(&msg, &mut engine.farm_conn, &mut engine.context, &engine.shared, &None, &mut engine.hb);
}

fn computations(shared: &SharedState) -> Vec<OptionComputation> {
    shared.market.take_generic_ticks(u64::MAX).into_iter()
        .filter(|g| g.code == market::COMPUTATIONS)
        .flat_map(|g| g.ticks)
        .filter_map(|t| if let GenTick::Option(c) = t { Some(c) } else { None })
        .collect()
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn an_option_stream_as_the_reference() {
    let shared = Arc::new(SharedState::new());
    // The features of the paper account of the recording: the last entries
    // are marked 2 (ibx#648).
    shared.reference.set_api_features(crate::control::logon::ApiFeatures::parse("ODDLOTBIDASK,PRCEXTRAPREC"));
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, mut farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.context.option_terms.insert(OPTION, terms());
    let id = engine.context.market.register(OPTION);
    engine.context.market.set_routing(id, "OPT", "SMART");
    let sub = farm::MdSubscribe {
        con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
        last_trade_date: "20261218".into(), strike: 340.0, right: "C".into(), multiplier: "100".into(),
        instrument: id, mode_9887: 0, snapshot: false,
    };
    engine.route_md_subscribe(&sub);

    // The option's pair, the underlying's pair, its model parameters, the
    // option's greeks (the reference's first four messages, 20:40:53.6).
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 4, "{msgs:#?}");
    assert!(msgs[0].ends_with("|263=1|146=2|262=1|6008=692808239|207=BEST|167=OPT|264=442|6088=Socket|9830=1|9839=1|262=2|6008=692808239|207=BEST|167=OPT|264=443|6088=Socket|9830=1|9839=2"), "{}", msgs[0]);
    assert!(msgs[1].ends_with("|263=1|146=2|262=3|6008=265598|207=BEST|167=CS|264=442|9839=1|262=4|6008=265598|207=BEST|167=CS|264=443|9839=2"), "{}", msgs[1]);
    assert!(msgs[2].ends_with("|263=1|146=1|262=5|6008=265598|207=IBVOL|167=CS|264=687"), "{}", msgs[2]);
    assert!(msgs[3].ends_with("|263=1|146=2|262=6|6008=692808239|207=IBVOL|167=OPT|264=732|262=7|6008=692808239|207=IBVOL|167=OPT|264=733"), "{}", msgs[3]);

    for (tag, req) in [(494084, 1), (494084, 2), (179, 3), (179, 4), (7178, 5), (494098, 6), (494099, 7)] {
        ack(&mut engine, tag, req);
    }
    let f = frames();
    // The model parameters release the status entries and the volatilities.
    generic(&mut engine, &f["params"]);
    engine.service_option_models(Instant::now());
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 2, "{msgs:#?}");
    assert!(msgs[0].ends_with("|263=1|146=1|262=8|6008=265598|207=BEST|167=CS|264=398"), "{}", msgs[0]);
    assert!(msgs[1].ends_with("|263=1|146=5|262=9|6008=692808239|207=BEST|167=OPT|264=398|262=10|6008=692808239|207=IBVOL|167=OPT|264=734\
        |262=11|6008=692808239|207=IBVOL|167=OPT|264=735|262=12|6008=692808239|207=IBVOL|167=OPT|264=736|262=13|6008=692808239|207=IBVOL|167=OPT|264=737"), "{}", msgs[1]);
    for (tag, req) in [(187, 8), (494136, 9), (30586, 10), (494137, 11), (494138, 12), (494139, 13)] {
        ack(&mut engine, tag, req);
    }
    generic(&mut engine, &f["greeks"]);
    generic(&mut engine, &f["vols"]);
    // The quotes of the recording at 20:40:56: AAPL 339.56 / 339.61, the
    // option 14.95 / 15.20, last 15.00.
    ticks(&mut engine, &[(179, &[(0, 33956), (1, 33961)]), (494084, &[(0, 1495), (1, 1520), (2, 1500)])]);

    // The model inputs as the reference had them (its model clock started
    // at 18:40:21.357 UTC).
    let clock = jiff::civil::date(2026, 10, 12).at(18, 40, 21, 357_000_000)
        .to_zoned(jiff::tz::TimeZone::UTC).unwrap().timestamp().as_millisecond();
    let base = inputs::midnight_ms(inputs::date_of_ms(clock, inputs::MODEL_TZ), inputs::MODEL_TZ);
    let curve = inputs::RateCurve::from_items(&inputs::parse_reference_xml(include_str!("../../../tests/fixtures/ibx502_rates.xml")), base);
    let divs = inputs::dividends_from_items(&inputs::parse_reference_xml(include_str!("../../../tests/fixtures/ibx502_dividends.xml")));
    engine.ccp.optcalc.set_inputs_for_test("USD", curve, AAPL, divs, clock, "Europe/Paris");

    // First pass: tick 13 from the server's greeks, then 10, 11, 12
    // without volatility or greeks (the reference at 20:40:55.14).
    let _ = computations(&shared);
    let now = Instant::now();
    engine.farm.optmd.force_pass_for_test(id, now);
    engine.service_option_models(now);
    let c = computations(&shared);
    assert_eq!(c.iter().map(|c| c.tick_type).collect::<Vec<_>>(), [13, 10, 11, 12]);
    assert_eq!(c[0], OptionComputation {
        req_id: 0, tick_type: 13, tick_attrib: 0, implied_vol: 0.24186426510722747, delta: 0.5450094232640754,
        opt_price: 15.082770319124098, pv_dividend: 0.2690576206220461, gamma: 0.011330915092391433,
        vega: 0.5755258897178326, theta: -0.1244593343138991, und_price: 339.662353515625,
    });
    assert_eq!((c[1].implied_vol, c[1].delta, c[1].opt_price, c[1].pv_dividend, c[1].und_price),
        (NOT_COMPUTED, NOT_COMPUTED, 14.949999809265137, 0.2690576206220461, 339.585));
    assert_eq!(c[2].opt_price, 15.199999809265137);
    assert_eq!(c[3].opt_price, 15.0);

    // Not due yet: nothing.
    engine.service_option_models(now);
    assert!(computations(&shared).is_empty());

    // Second pass: 10, 11, 12 with the server's volatilities and the local
    // greeks; tick 13 did not change. The reference's at 20:40:56.16
    // (underlying 339.585): bid delta 0.5438673031565397, gamma
    // 0.011444174820836593, vega 0.5755335220323161, theta
    // -0.12403757377692223.
    engine.farm.optmd.force_pass_for_test(id, now);
    engine.service_option_models(now);
    let c = computations(&shared);
    assert_eq!(c.iter().map(|c| c.tick_type).collect::<Vec<_>>(), [10, 11, 12]);
    assert_eq!((c[0].implied_vol, c[1].implied_vol, c[2].implied_vol), (0.2395566468611969, 0.24390056272714186, 0.24192026245757572));
    assert!(close(c[0].delta, 0.5438673031565397, 1e-9) && close(c[0].gamma, 0.011444174820836593, 1e-10), "{:?}", c[0]);
    assert!(close(c[0].vega, 0.5755335220323161, 1e-8) && close(c[0].theta, -0.12403757377692223, 1e-9), "{:?}", c[0]);
    assert!(close(c[1].delta, 0.5438208667936552, 1e-9) && close(c[2].delta, 0.5438401922879619, 1e-9), "{:?}", c);

    // The cancel, in the reference's order (20:41:05.5): the underlying's
    // status, the option's pair, the underlying's pair, the option's
    // entries with its status last, the model parameters.
    engine.route_md_cancel(id);
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 5, "{msgs:#?}");
    assert!(msgs[0].ends_with("|263=2|146=1|262=8|6008=265598|207=BEST|167=CS|264=398"), "{}", msgs[0]);
    assert!(msgs[1].contains("|263=2|146=2|262=1|6008=692808239|207=BEST|167=OPT|264=442|"), "{}", msgs[1]);
    assert!(msgs[2].ends_with("|263=2|146=2|262=3|6008=265598|207=BEST|167=CS|264=442|9839=1|262=4|6008=265598|207=BEST|167=CS|264=443|9839=2"), "{}", msgs[2]);
    let codes: Vec<&str> = msgs[3].split('|').filter_map(|f| f.strip_prefix("264=")).collect();
    assert_eq!(codes, ["737", "736", "735", "734", "733", "732", "398"]);
    assert!(msgs[4].ends_with("|263=2|146=1|262=5|6008=265598|207=IBVOL|167=CS|264=687"), "{}", msgs[4]);
    assert!(engine.farm.optmd.is_empty());

    // The option asked again (ibx#646; the reference 10.51, paper,
    // 14/10/2026 regular hours: a request after an earlier one of the same
    // call computes 0.15 s after its first data, from the model values kept,
    // with the volatilities and greeks, before its own values come).
    let _ = computations(&shared);
    engine.route_md_subscribe(&sub);
    let msgs = sent(&mut farm_side);
    let req_of = |code: &str, con: &str| -> u32 {
        msgs.iter().find_map(|m| {
            let f: Vec<&str> = m.split('|').collect();
            f.windows(5).find(|w| w[1] == format!("6008={con}") && w[4] == format!("264={code}"))
                .and_then(|w| w[0].strip_prefix("262=")).map(|r| r.parse().unwrap())
        }).unwrap_or_else(|| panic!("{code} {con} {msgs:#?}"))
    };
    let (under_req, greeks_req) = (req_of("442", "265598"), req_of("732", "692808239"));
    let now = Instant::now();
    // The session's clock ticks 1.7 s after `now` (ibx#684).
    engine.farm.optmd.set_clock_for_test(now - Duration::from_millis(20_300));
    engine.service_option_models(now);
    engine.service_option_models(now + Duration::from_millis(100));
    assert!(computations(&shared).is_empty());
    engine.service_option_models(now + Duration::from_millis(160));
    let c = computations(&shared);
    assert_eq!(c.iter().map(|c| c.tick_type).collect::<Vec<_>>(), [13, 10, 11, 12]);
    assert_eq!(c[0].implied_vol, 0.24186426510722747);
    assert_eq!((c[1].implied_vol, c[2].implied_vol, c[3].implied_vol), (0.2395566468611969, 0.24390056272714186, 0.24192026245757572));
    assert!(close(c[1].delta, 0.5438673031565397, 1e-6) && c[1].und_price == 339.585, "{:?}", c[1]);
    // The model's own greeks: a pass at once, here with the underlying's
    // new quote (the reference 10.51, 15/10/2026: 0.26 s after the first).
    ack(&mut engine, 179, under_req);
    ack(&mut engine, 494098, greeks_req);
    ticks(&mut engine, &[(179, &[(0, 33960), (1, 33965)])]);
    generic(&mut engine, &f["greeks"]);
    engine.service_option_models(now + Duration::from_millis(400));
    let c = computations(&shared);
    assert_eq!(c.iter().map(|c| c.tick_type).collect::<Vec<_>>(), [10, 11, 12]);
    assert_eq!(c[0].und_price, 339.625);
    // Then the session's clock: nothing before its tick.
    engine.service_option_models(now + Duration::from_millis(1300));
    assert!(computations(&shared).is_empty());

    // A snapshot of it then: 13, 10 and 11 from the kept values at once,
    // 12 at the clock's next tick (ibx#646, ibx#684; the reference 10.51,
    // observed in the recordings of 13-16/10/2026: tick 12 at the first even
    // second after the first data, then the end).
    engine.route_md_cancel(id);
    let mut snap = sub.clone();
    snap.snapshot = true;
    engine.route_md_subscribe(&snap);
    let _ = sent(&mut farm_side);
    let _ = computations(&shared);
    let now = Instant::now();
    engine.farm.optmd.set_clock_for_test(now - Duration::from_millis(20_300));
    engine.service_option_models(now);
    engine.service_option_models(now + Duration::from_millis(160));
    assert_eq!(computations(&shared).iter().map(|c| c.tick_type).collect::<Vec<_>>(), [13, 10, 11]);
    engine.service_option_models(now + Duration::from_millis(1200));
    assert!(computations(&shared).is_empty(), "the last waits for the tick");
    engine.service_option_models(now + Duration::from_millis(1700));
    assert_eq!(computations(&shared).iter().map(|c| c.tick_type).collect::<Vec<_>>(), [12]);

    // A stream again, whose own greeks come before its first pass: the
    // second pass right after the first (the reference 10.51, 14/10/2026:
    // the two in the same millisecond).
    engine.route_md_cancel(id);
    let _ = sent(&mut farm_side);
    engine.route_md_subscribe(&sub);
    let msgs = sent(&mut farm_side);
    let req_of = |code: &str, con: &str| -> u32 {
        msgs.iter().find_map(|m| {
            let f: Vec<&str> = m.split('|').collect();
            f.windows(5).find(|w| w[1] == format!("6008={con}") && w[4] == format!("264={code}"))
                .and_then(|w| w[0].strip_prefix("262=")).map(|r| r.parse().unwrap())
        }).unwrap_or_else(|| panic!("{code} {con} {msgs:#?}"))
    };
    ack(&mut engine, 179, req_of("442", "265598"));
    ack(&mut engine, 494098, req_of("732", "692808239"));
    let _ = computations(&shared);
    let now = Instant::now();
    engine.service_option_models(now);
    generic(&mut engine, &f["greeks"]);
    engine.service_option_models(now + Duration::from_millis(160));
    assert_eq!(computations(&shared).iter().map(|c| c.tick_type).collect::<Vec<_>>(), [13, 10, 11, 12]);
    ticks(&mut engine, &[(179, &[(0, 33970), (1, 33975)])]);
    engine.service_option_models(now + Duration::from_millis(161));
    let c = computations(&shared);
    assert_eq!(c.iter().map(|c| c.tick_type).collect::<Vec<_>>(), [10, 11, 12]);
    assert_eq!(c[0].und_price, 339.725);
}

#[test]
fn a_snapshot_with_known_parameters_sends_its_entries_at_once() {
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, mut farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.context.option_terms.insert(OPTION, terms());
    let id = engine.context.market.register(OPTION);
    engine.context.market.set_routing(id, "OPT", "SMART");
    let mut sub = farm::MdSubscribe {
        con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
        last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(),
        instrument: id, mode_9887: 0, snapshot: false,
    };
    // A stream first, whose model parameters came; it ends.
    engine.route_md_subscribe(&sub);
    for (tag, req) in [(494084, 1), (494084, 2), (179, 3), (179, 4), (7178, 5)] {
        ack(&mut engine, tag, req);
    }
    generic(&mut engine, &frames()["params"]);
    engine.route_md_cancel(id);
    let _ = sent(&mut farm_side);

    // The snapshot (the reference at 20:41:06.65): its pair, the model's
    // pair of the option, the underlying's pair, the option's entries, the
    // parameters and status of the underlying.
    sub.snapshot = true;
    engine.route_md_subscribe(&sub);
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 5, "{msgs:#?}");
    assert!(msgs[0].contains("|263=3|146=2|") && msgs[0].contains("|264=442|"), "{}", msgs[0]);
    assert!(msgs[1].contains("|263=1|146=2|") && msgs[1].contains("|167=OPT|264=442|9830=1|") && !msgs[1].contains("6088"), "{}", msgs[1]);
    assert!(msgs[2].contains("|263=1|146=2|") && msgs[2].contains("|6008=265598|207=BEST|167=CS|264=442|"), "{}", msgs[2]);
    let codes: Vec<&str> = msgs[3].split('|').filter_map(|f| f.strip_prefix("264=")).collect();
    assert_eq!(codes, ["398", "732", "733", "734", "735", "736", "737"]);
    let codes: Vec<&str> = msgs[4].split('|').filter_map(|f| f.strip_prefix("264=")).collect();
    assert_eq!(codes, ["687", "398"]);
    let snapshot_ids: Vec<String> = msgs[0].split('|').filter_map(|f| f.strip_prefix("262=")).map(String::from).collect();

    // Its end, in the reference's order (20:41:08.17).
    engine.route_md_cancel(id);
    let msgs = sent(&mut farm_side);
    // The snapshot's own entries are not cancelled (ibx#696; the reference
    // 10.51, paper, five recordings of 13/10 and 14/10/2026).
    for m in &msgs {
        let ids: Vec<&str> = m.split('|').filter_map(|f| f.strip_prefix("262=")).collect();
        assert!(snapshot_ids.iter().all(|s| !ids.contains(&s.as_str())), "{m}");
    }
    let codes: Vec<Vec<&str>> = msgs.iter().map(|m| m.split('|').filter_map(|f| f.strip_prefix("264=")).collect()).collect();
    assert_eq!(codes[..5], [vec!["733", "732"], vec!["398", "687"], vec!["442", "443"], vec!["442", "443"], vec!["737", "736", "735", "734", "398"]],
        "{msgs:#?}");
}

// The reference 10.51 after hours (paper, 12/10/2026, 22:28 Paris), the
// option's status frozen: a stream got no computation; a snapshot got the
// model's at once (with the mid volatility) and the sides' at its time
// limit.
#[test]
fn a_frozen_option_keeps_its_computations() {
    for snapshot in [false, true] {
        let shared = Arc::new(SharedState::new());
        let mut engine = HotLoop::new(shared.clone(), None, None);
        engine.set_farm_name("usfarm".into());
        let (client, mut farm_side) = crate::protocol::connection::mem_pair();
        engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
        engine.context.option_terms.insert(OPTION, terms());
        let id = engine.context.market.register(OPTION);
        engine.context.market.set_routing(id, "OPT", "SMART");
        let sub = farm::MdSubscribe {
            con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
            last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(),
            instrument: id, mode_9887: 0, snapshot,
        };
        engine.route_md_subscribe(&sub);
        let first = engine.farm.next_md_req_id;
        let _ = sent(&mut farm_side);
        // The ids of the model's entries, in the order they went: (option
        // pair for a snapshot), underlying pair, parameters, greeks.
        let mut req = first - if snapshot { 7 } else { 5 };
        if snapshot {
            ack(&mut engine, 494084, req);
            ack(&mut engine, 494084, req + 1);
            req += 2;
        }
        for (tag, r) in [(179, req), (179, req + 1), (7178, req + 2), (494098, req + 3), (494099, req + 4)] {
            ack(&mut engine, tag, r);
        }
        let f = frames();
        generic(&mut engine, &f["params"]);
        engine.service_option_models(Instant::now());
        let _ = sent(&mut farm_side);
        let next = engine.farm.next_md_req_id;
        for (tag, r) in [(187, next - 6), (494136, next - 5), (30586, next - 4), (494137, next - 3), (494138, next - 2), (494139, next - 1)] {
            ack(&mut engine, tag, r);
        }
        // The option's status: frozen.
        generic(&mut engine, &[0x00, 0x48, 0x00, 0x07, 0x8a, 0x38, 0x04, 0x00, 0x00, 0x00, 0x01]);
        generic(&mut engine, &f["greeks"]);
        generic(&mut engine, &f["vols"]);
        let _ = shared.market.take_generic_ticks(u64::MAX);
        let now = Instant::now();
        engine.farm.optmd.force_pass_for_test(id, now);
        engine.service_option_models(now);
        let ticks = shared.market.take_generic_ticks(u64::MAX);
        let of = |code: i32| -> Vec<i32> {
            ticks.iter().filter(|g| g.code == code).flat_map(|g| g.ticks.iter())
                .filter_map(|t| if let GenTick::Option(c) = t { Some(c.tick_type) } else { None }).collect()
        };
        if snapshot {
            assert_eq!(of(market::COMPUTATIONS), [13]);
            assert_eq!(of(market::COMPUTATIONS_KEPT), [10, 11, 12]);
        } else {
            assert!(of(market::COMPUTATIONS).is_empty());
            assert_eq!(of(market::COMPUTATIONS_KEPT), [13, 10, 11, 12]);
        }
        let model = ticks.iter().flat_map(|g| g.ticks.iter())
            .find_map(|t| if let GenTick::Option(c) = t { (c.tick_type == 13).then_some(*c) } else { None }).unwrap();
        // The snapshot's model volatility is the mid's, the stream's the model's.
        assert_eq!(model.implied_vol, if snapshot { 0.24312244477943712 } else { 0.24186426510722747 });
        let side = ticks.iter().flat_map(|g| g.ticks.iter())
            .find_map(|t| if let GenTick::Option(c) = t { (c.tick_type == 10).then_some(*c) } else { None }).unwrap();
        assert_eq!((side.implied_vol, side.delta), (0.2395566468611969, NOT_COMPUTED), "the volatility, no greeks");
        let last = ticks.iter().flat_map(|g| g.ticks.iter())
            .find_map(|t| if let GenTick::Option(c) = t { (c.tick_type == 12).then_some(*c) } else { None }).unwrap();
        assert_eq!(last.implied_vol, NOT_COMPUTED, "the last without its volatility");
    }
}

// ibx#664: the underlying's exchange map is asked at the acknowledgement of
// the model's bid and ask entry of the underlying, once per key, as the
// reference 10.51 asked AAPL's in a fresh process (13/10/2026, regular
// hours) before AAPL itself was asked.
#[test]
fn the_model_asks_the_underlyings_exchange_map() {
    let shared = Arc::new(SharedState::new());
    shared.reference.set_api_features(crate::control::logon::ApiFeatures::parse("ODDLOTBIDASK,PRCEXTRAPREC"));
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, mut farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.context.option_terms.insert(OPTION, terms());
    let id = engine.context.market.register(OPTION);
    engine.context.market.set_routing(id, "OPT", "SMART");
    engine.route_md_subscribe(&farm::MdSubscribe {
        con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
        last_trade_date: "20261218".into(), strike: 340.0, right: "C".into(), multiplier: "100".into(),
        instrument: id, mode_9887: 0, snapshot: false,
    });
    assert_eq!(sent(&mut farm_side).len(), 4);
    let ack_with = |engine: &mut HotLoop, tag: u32, req: u32| {
        let msg = format!("8=O\x0135=Q\x01{tag},{req},0.01,0,3,9c,,1,1");
        engine.farm.process_farm_message(msg.as_bytes(), &mut engine.farm_conn, &mut engine.context, &engine.shared, &None, &mut engine.hb);
    };
    ack_with(&mut engine, 179, 3);
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 1, "{msgs:#?}");
    assert!(msgs[0].contains("|6008=265598|207=BEST|167=CS|264=626"), "{}", msgs[0]);
    // The last entry's acknowledgement does not ask it again.
    ack_with(&mut engine, 179, 4);
    assert!(sent(&mut farm_side).is_empty());
}

// ibx#647, ibx#494: the option's status frozen, a request in real-time mode.
// The reference 10.51 after hours (13/10/2026, two streams and two
// snapshots of AAPL calls; the same on 12/10 and 14/10/2026): the option's
// frozen pair, then the entry after it, each in its message; their cancels
// in the request's own order; the frozen pair's trade given again to the
// streams. Not asked in frozen mode, whose requests have their own frozen
// pair (ibx#447).
#[test]
fn a_frozen_option_asks_its_frozen_pair() {
    for (snapshot, frozen_mode) in [(false, false), (true, false), (false, true)] {
        let shared = Arc::new(SharedState::new());
        shared.reference.set_api_features(crate::control::logon::ApiFeatures::parse("ODDLOTBIDASK,PRCEXTRAPREC"));
        let mut engine = HotLoop::new(shared.clone(), None, None);
        engine.set_farm_name("usfarm".into());
        engine.farm.md_modes.frozen = frozen_mode;
        let (client, mut farm_side) = crate::protocol::connection::mem_pair();
        engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
        engine.context.option_terms.insert(OPTION, terms());
        let id = engine.context.market.register(OPTION);
        engine.context.market.set_routing(id, "OPT", "SMART");
        let sub = farm::MdSubscribe {
            con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
            last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(),
            instrument: id, mode_9887: 0, snapshot,
        };
        engine.route_md_subscribe(&sub);
        let first = engine.farm.next_md_req_id;
        let _ = sent(&mut farm_side);
        let mut req = first - if snapshot { 7 } else { 5 };
        if snapshot {
            ack(&mut engine, 494084, req);
            ack(&mut engine, 494084, req + 1);
            req += 2;
        }
        for (tag, r) in [(179, req), (179, req + 1), (7178, req + 2), (494098, req + 3), (494099, req + 4)] {
            ack(&mut engine, tag, r);
        }
        generic(&mut engine, &frames()["params"]);
        engine.service_option_models(Instant::now());
        let _ = sent(&mut farm_side);
        let next = engine.farm.next_md_req_id;
        for (tag, r) in [(187, next - 6), (494136, next - 5), (30586, next - 4), (494137, next - 3), (494138, next - 2), (494139, next - 1)] {
            ack(&mut engine, tag, r);
        }
        // The option's status: frozen.
        generic(&mut engine, &[0x00, 0x48, 0x00, 0x07, 0x8a, 0x38, 0x04, 0x00, 0x00, 0x00, 0x01]);
        engine.service_option_models(Instant::now());
        let msgs = sent(&mut farm_side);
        if frozen_mode {
            assert!(msgs.is_empty(), "{msgs:#?}");
            continue;
        }
        let n = next;
        assert_eq!(msgs, [
            format!("35=V|263=1|146=2|262={n}|6008=692808239|207=BEST|167=OPT|264=442|9887=2|9830=1|9839=1\
                |262={}|6008=692808239|207=BEST|167=OPT|264=443|9887=2|9830=1|9839=2", n + 1),
            format!("35=V|263=1|146=1|262={}|6008=692808239|207=BEST|167=OPT|264=317", n + 2),
        ], "snapshot {snapshot}");
        // The status again: nothing more.
        generic(&mut engine, &[0x00, 0x48, 0x00, 0x07, 0x8a, 0x38, 0x04, 0x00, 0x00, 0x00, 0x01]);
        engine.service_option_models(Instant::now());
        assert!(sent(&mut farm_side).is_empty());

        // The frozen pair's image repeats the option's last trade: a step
        // for its requests (the reference gave the size and time again).
        ack(&mut engine, 988376, n);
        ack(&mut engine, 988376, n + 1);
        ack(&mut engine, 652179, n + 2);
        let queue = &shared.market.md_events;
        queue.listen(id, true);
        let from = queue.head();
        ticks(&mut engine, &[(988376, &[(2, 4600), (6, 1)])]);
        let steps: Vec<MdStep> = (from..queue.head()).map(|s| queue.step_at(s).0).collect();
        assert_eq!(steps, [MdStep::Again], "snapshot {snapshot}");

        // The cancels: the frozen pair before the option's pair; the entry
        // after it with the greeks of a snapshot, last of the option's
        // entries of a stream.
        engine.route_md_cancel(id);
        let msgs = sent(&mut farm_side);
        let codes: Vec<Vec<String>> = msgs.iter().map(|m| {
            m.split('|').filter_map(|f| f.strip_prefix("264=").map(str::to_string))
                .chain(m.contains("9887=2").then(|| "frozen".to_string())).collect()
        }).collect();
        let expected: Vec<Vec<&str>> = if snapshot {
            vec![vec!["733", "732", "317"], vec!["398", "687"], vec!["442", "443", "frozen"], vec!["442", "443"], vec!["442", "443"],
                vec!["737", "736", "735", "734", "398"]]
        } else {
            vec![vec!["398"], vec!["442", "443", "frozen"], vec!["442", "443"], vec!["442", "443"],
                vec!["737", "736", "735", "734", "733", "732", "398", "317"], vec!["687"]]
        };
        assert_eq!(codes[..expected.len()], expected, "snapshot {snapshot}: {msgs:#?}");
    }
}

// ibx#647: a second option of the same underlying asks the entry after its
// frozen pair on the first option that asked it, and cancels it alone, last
// (the reference 10.51 after hours, 13/10 and 14/10/2026: six requests of
// the AAPL 300 call asked it on the 340 call).
#[test]
fn the_entry_after_the_frozen_pair_stays_on_the_first_option() {
    const SECOND: i64 = 692808040;
    let shared = Arc::new(SharedState::new());
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, mut farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.context.option_terms.insert(OPTION, terms());
    engine.context.option_terms.insert(SECOND, OptionTerms { con_id: SECOND, strike: 300.0, ..terms() });
    let mut last_after = String::new();
    for (con_id, tag) in [(OPTION, 494136u32), (SECOND, 494236u32)] {
        let id = engine.context.market.register(con_id);
        engine.context.market.set_routing(id, "OPT", "SMART");
        let sub = farm::MdSubscribe {
            con_id, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
            last_trade_date: String::new(), strike: 0.0, right: String::new(), multiplier: String::new(),
            instrument: id, mode_9887: 0, snapshot: false,
        };
        engine.route_md_subscribe(&sub);
        let _ = sent(&mut farm_side);
        // The option's status entry: the one with code 398 of the option.
        let status_req = engine.farm.optmd.models.iter().find(|m| m.instrument == id)
            .and_then(|m| m.entries.iter().find(|e| !e.under && e.code == 398).map(|e| e.farm_req)).unwrap();
        let req = if status_req == 0 {
            // Parameters not known yet: they come, the status goes out.
            let first = engine.farm.next_md_req_id;
            ack(&mut engine, 7178, first - 3);
            generic(&mut engine, &frames()["params"]);
            engine.service_option_models(Instant::now());
            let _ = sent(&mut farm_side);
            engine.farm.optmd.models.iter().find(|m| m.instrument == id)
                .and_then(|m| m.entries.iter().find(|e| !e.under && e.code == 398).map(|e| e.farm_req)).unwrap()
        } else {
            status_req
        };
        ack(&mut engine, tag, req);
        let [b0, b1, b2, b3] = tag.to_be_bytes();
        generic(&mut engine, &[0x00, 0x48, b0, b1, b2, b3, 0x04, 0x00, 0x00, 0x00, 0x01]);
        engine.service_option_models(Instant::now());
        let msgs = sent(&mut farm_side);
        assert_eq!(msgs.len(), 2, "{msgs:#?}");
        assert!(msgs[1].ends_with(&format!("|6008={OPTION}|207=BEST|167=OPT|264=317")), "{}", msgs[1]);
        engine.route_md_cancel(id);
        let msgs = sent(&mut farm_side);
        last_after = msgs.last().cloned().unwrap_or_default();
        if con_id == OPTION {
            assert!(msgs.iter().any(|m| m.contains("|264=398|") && m.ends_with("|264=317")), "{msgs:#?}");
        }
    }
    assert!(last_after.ends_with(&format!("|263=2|146=1|262={}|6008={OPTION}|207=BEST|167=OPT|264=317", engine.farm.next_md_req_id - 1)),
        "{last_after}");
}

/// An engine on the primary farm with the AAPL call's terms, its slot
/// registered; the farm's side.
fn order_engine() -> (HotLoop, Arc<SharedState>, crate::protocol::connection::MemTransport, InstrumentId) {
    let shared = Arc::new(SharedState::new());
    shared.reference.set_api_features(crate::control::logon::ApiFeatures::parse("ODDLOTBIDASK,PRCEXTRAPREC"));
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.context.option_terms.insert(OPTION, terms());
    let id = engine.context.market.register(OPTION);
    engine.context.market.set_routing(id, "OPT", "SMART");
    (engine, shared, farm_side, id)
}

// ibx#693 (the reference 10.51, paper, 16/10/2026 regular hours, a fresh
// session, `.tmp/673/wire_gw_vol.jsonl`): an order on an AAPL call starts
// the option's market data for the engine: the underlying's pair and model
// parameters, then the option's own pair with no API mark and its greeks;
// once the parameters came, the statuses and the volatilities. Its
// computations go to no one. A second order of the call starts nothing; a
// client's request of the call then takes its place.
#[test]
fn an_option_order_starts_the_option_model_for_the_engine() {
    let (mut engine, shared, mut farm_side, id) = order_engine();
    engine.context.order_models.push((id, OPTION, "SMART".into()));
    engine.service_option_models(Instant::now());
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 4, "{msgs:#?}");
    assert!(msgs[0].ends_with("|263=1|146=2|262=1|6008=265598|207=BEST|167=CS|264=442|9839=1|262=2|6008=265598|207=BEST|167=CS|264=443|9839=2"), "{}", msgs[0]);
    assert!(msgs[1].ends_with("|263=1|146=1|262=3|6008=265598|207=IBVOL|167=CS|264=687"), "{}", msgs[1]);
    assert!(msgs[2].ends_with("|263=1|146=2|262=4|6008=692808239|207=BEST|167=OPT|264=442|9839=1|262=5|6008=692808239|207=BEST|167=OPT|264=443|9839=2"), "{}", msgs[2]);
    assert!(msgs[3].ends_with("|263=1|146=2|262=6|6008=692808239|207=IBVOL|167=OPT|264=732|262=7|6008=692808239|207=IBVOL|167=OPT|264=733"), "{}", msgs[3]);
    assert!(engine.farm.optmd.is_order_model(id));

    for (tag, req) in [(179, 1), (179, 2), (7178, 3), (494084, 4), (494084, 5), (494098, 6), (494099, 7)] {
        ack(&mut engine, tag, req);
    }
    let f = frames();
    generic(&mut engine, &f["params"]);
    engine.service_option_models(Instant::now());
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 2, "{msgs:#?}");
    assert!(msgs[0].ends_with("|263=1|146=1|262=8|6008=265598|207=BEST|167=CS|264=398"), "{}", msgs[0]);
    let codes: Vec<&str> = msgs[1].split('|').filter_map(|f| f.strip_prefix("264=")).collect();
    assert_eq!(codes, ["398", "734", "735", "736", "737"]);
    for (tag, req) in [(187, 8), (494136, 9), (30586, 10), (494137, 11), (494138, 12), (494139, 13)] {
        ack(&mut engine, tag, req);
    }
    generic(&mut engine, &f["greeks"]);
    generic(&mut engine, &f["vols"]);
    ticks(&mut engine, &[(179, &[(0, 33956), (1, 33961)]), (494084, &[(0, 1495), (1, 1520), (2, 1500)])]);
    // The option's own pair feeds the quote of its slot.
    assert_eq!(engine.context.quote(id).bid, 1495 * crate::types::PRICE_SCALE / 100);
    let now = Instant::now();
    engine.farm.optmd.force_pass_for_test(id, now);
    engine.service_option_models(now);
    engine.service_option_models(now + Duration::from_secs(3));
    assert!(shared.market.take_generic_ticks(u64::MAX).is_empty(), "no computations for a client");
    assert!(sent(&mut farm_side).is_empty(), "kept, no cancel");

    // A second order of the call: nothing more.
    engine.context.order_models.push((id, OPTION, "SMART".into()));
    engine.service_option_models(Instant::now());
    assert!(sent(&mut farm_side).is_empty());
    assert_eq!(engine.farm.optmd.models.len(), 1);

    // A client's request of the call: the order's model gives way to the
    // request's.
    let sub = farm::MdSubscribe {
        con_id: OPTION, symbol: "AAPL".into(), exchange: "SMART".into(), sec_type: "OPT".into(),
        last_trade_date: "20261218".into(), strike: 340.0, right: "C".into(), multiplier: "100".into(),
        instrument: id, mode_9887: 0, snapshot: false,
    };
    engine.route_md_subscribe(&sub);
    let msgs = sent(&mut farm_side);
    assert!(msgs.iter().any(|m| m.contains("|263=2|146=2|262=4|6008=692808239|207=BEST|167=OPT|264=442|9839=1|262=5|")), "{msgs:#?}");
    assert!(msgs.iter().any(|m| m.contains("|263=1|") && m.contains("|167=OPT|264=442|6088=Socket|9830=1|")), "{msgs:#?}");
    assert_eq!(engine.farm.optmd.models.len(), 1);
    assert!(!engine.farm.optmd.is_order_model(id));
}

// ibx#693: an order's model whose option's definition is not known asks
// it, and starts once it came.
#[test]
fn an_order_model_waits_for_the_option_definition() {
    let (mut engine, _shared, mut farm_side, id) = order_engine();
    engine.context.option_terms.clear();
    let (ccp, mut ccp_side) = crate::protocol::connection::mem_pair();
    engine.ccp_conn = Some(crate::protocol::connection::Connection::new_mem(ccp));
    engine.context.order_models.push((id, OPTION, "SMART".into()));
    engine.service_option_models(Instant::now());
    assert!(sent(&mut farm_side).is_empty());
    use std::io::Read;
    ccp_side.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let mut buf = [0u8; 4096];
    let n = ccp_side.read(&mut buf).unwrap();
    let asked = String::from_utf8_lossy(&buf[..n]).replace('\x01', "|");
    assert!(asked.contains("|35=c|") && asked.contains(&format!("|6008={OPTION}|")), "{asked}");
    engine.service_option_models(Instant::now());
    assert!(sent(&mut farm_side).is_empty(), "still waiting");
    engine.context.option_terms.insert(OPTION, terms());
    engine.service_option_models(Instant::now());
    assert_eq!(sent(&mut farm_side).len(), 4);
}

// ibx#693 (the reference 10.51, paper, 16/10/2026 regular hours: 2119
// "connecting:usopt" 1.09 s after the order, 2104 at 1.59 s): the order's
// model connects the option farm; the engine started that connection, so
// no notice at its start; the check 0.5 s after its first queued
// subscription gives it (ibx#684); the logon gives the farm's OK and the
// queued entries go out.
#[test]
fn an_option_order_connects_the_option_farm_with_its_notices() {
    use std::sync::atomic::AtomicBool;
    let (mut engine, _shared, mut farm_side, id) = order_engine();
    engine.set_routing_table(crate::engine::routing::TableKind::MarketData,
        "BEST,STK,Top,1,*,cdc1.example,4000,usfarm;BEST,OPT,Top,1,*,cdc1.example,4000,usopt");
    engine.context.agg_groups.insert(AAPL, 1);
    engine.context.agg_groups.insert(OPTION, 1);
    let t0 = Instant::now();
    engine.context.order_models.push((id, OPTION, "SMART".into()));
    engine.service_option_models(t0);
    let msgs = sent(&mut farm_side);
    assert_eq!(msgs.len(), 2, "the underlying's entries on the primary farm: {msgs:#?}");
    let usopt = engine.pool.find("usopt").expect("the option farm opened on demand");
    assert_eq!(engine.pool.get(usopt).unwrap().queued(), 2, "the option's pair and greeks wait for the logon");
    assert_eq!(engine.pool.due_connects(Instant::now()), [usopt]);

    let (tx, rx) = crossbeam_channel::bounded(1);
    engine.pool.set_connecting(usopt, rx, Arc::new(AtomicBool::new(true)));
    let connecting = |engine: &mut HotLoop, at: Instant| engine.pool.poll_connects(at).1.iter()
        .filter(|e| matches!(e, pool::FarmEvent::Connecting(_))).count();
    assert_eq!(connecting(&mut engine, t0), 0, "none at the start of a connection the engine began");
    assert_eq!(connecting(&mut engine, t0 + Duration::from_millis(600)), 1, "the check");
    assert_eq!(connecting(&mut engine, t0 + Duration::from_millis(1200)), 0, "nothing new queued");
    assert_eq!(pool::farm_notice(pool::FarmKind::MarketData, &pool::FarmEvent::Connecting(usopt), "usopt"),
        (2119, "Market data farm is connecting:usopt".to_string()));

    let (client, mut opt_side) = crate::protocol::connection::mem_pair();
    tx.send(Ok(crate::protocol::connection::Connection::new_mem(client))).unwrap();
    let (done, _) = engine.pool.poll_connects(t0 + Duration::from_millis(1300));
    assert_eq!(done.len(), 1);
    let (farm, conn) = done.into_iter().next().unwrap();
    let event = engine.pool.on_connected(farm, conn.unwrap(), Instant::now());
    assert_eq!(pool::farm_notice(pool::FarmKind::MarketData, &event, "usopt"),
        (2104, "Market data farm connection is OK:usopt".to_string()));
    let flushed = sent(&mut opt_side);
    assert_eq!(flushed.len(), 2, "{flushed:#?}");
    assert!(flushed[0].contains("|167=OPT|264=442|9839=1|") && !flushed[0].contains("9830"), "{}", flushed[0]);
    assert!(flushed[1].contains("|167=OPT|264=732|"), "{}", flushed[1]);
    // The model's entries keep the farm up.
    assert!(engine.farm.optmd.uses_farm(usopt));
}

// ibx#666 (the reference 10.51, paper, regular hours 16/10/2026,
// `.tmp/666/wire_gw_types3.jsonl` and `wire_gw_types4.jsonl`, VFIAX bought
// by cash on FUNDSERV): the first fund order of the session asks the fund's
// bid and ask pair on the order's exchange for the engine, alone, on the
// fund's farm: the entry marks, no API source; no option model. The farm's
// connecting notice goes at once at the start of its connect and not again
// while it connects (one notice, the farm's OK 1.2 s and 2.3 s later); its
// OK at the logon. The pair is kept and keeps the farm up; a second order
// of the fund asks nothing.
#[test]
fn a_fund_order_asks_the_fund_top_of_book_for_the_engine() {
    use std::sync::atomic::AtomicBool;
    const VFIAX: i64 = 48013549;
    let shared = Arc::new(SharedState::new());
    shared.reference.set_api_features(crate::control::logon::ApiFeatures::parse("ODDLOTBIDASK,PRCEXTRAPREC"));
    let mut engine = HotLoop::new(shared.clone(), None, None);
    engine.set_farm_name("usfarm".into());
    let (client, mut farm_side) = crate::protocol::connection::mem_pair();
    engine.farm_conn = Some(crate::protocol::connection::Connection::new_mem(client));
    engine.set_routing_table(crate::engine::routing::TableKind::MarketData,
        "BEST,STK,Top,1,*,cdc1.example,4000,usfarm;FUNDSERV,FUND,Top,-1,*,cdc1.example,4000,usbond");
    let id = engine.context.market.register(VFIAX);
    engine.context.market.set_routing(id, "FUND", "FUNDSERV");
    let t0 = Instant::now();
    engine.context.order_models.push((id, VFIAX, "FUNDSERV".into()));
    engine.service_option_models(t0);
    assert!(sent(&mut farm_side).is_empty(), "nothing on the main farm");
    assert!(engine.farm.optmd.is_empty() && !engine.farm.optmd.has(id), "no option model");
    let usbond = engine.pool.find("usbond").expect("the fund's farm opened on demand");
    assert_eq!(engine.pool.get(usbond).unwrap().queued(), 1, "the pair waits for the logon");
    assert_eq!(engine.pool.due_connects(Instant::now()), [usbond]);

    let (tx, rx) = crossbeam_channel::bounded(1);
    engine.pool.set_connecting(usbond, rx, Arc::new(AtomicBool::new(true)));
    let connecting = |engine: &mut HotLoop, at: Instant| engine.pool.poll_connects(at).1.iter()
        .filter(|e| matches!(e, pool::FarmEvent::Connecting(_))).count();
    assert_eq!(connecting(&mut engine, t0), 1, "at the start of the connect");
    assert_eq!(connecting(&mut engine, t0 + Duration::from_millis(600)), 0, "not again for the same pair");
    assert_eq!(connecting(&mut engine, t0 + Duration::from_millis(1200)), 0);

    let (client, mut bond_side) = crate::protocol::connection::mem_pair();
    tx.send(Ok(crate::protocol::connection::Connection::new_mem(client))).unwrap();
    let (done, _) = engine.pool.poll_connects(t0 + Duration::from_millis(1300));
    let (farm, conn) = done.into_iter().next().unwrap();
    let event = engine.pool.on_connected(farm, conn.unwrap(), Instant::now());
    assert_eq!(pool::farm_notice(pool::FarmKind::MarketData, &event, "usbond"),
        (2104, "Market data farm connection is OK:usbond".to_string()));
    let flushed = sent(&mut bond_side);
    assert_eq!(flushed.len(), 1, "{flushed:#?}");
    assert!(flushed[0].ends_with(concat!("|263=1|146=2|262=1|6008=48013549|207=FUNDSERV|167=FUND|264=442|9839=1",
        "|262=2|6008=48013549|207=FUNDSERV|167=FUND|264=443|9839=2")), "{}", flushed[0]);
    assert!(engine.farm.order_quotes_use_farm(usbond), "kept, the farm stays up");

    // Its acknowledgements on the fund's farm keep their tags.
    engine.farm.rx_farm = usbond;
    ack(&mut engine, 6510, 1);
    ack(&mut engine, 6510, 2);
    engine.farm.rx_farm = PRIMARY_MD;
    assert!(engine.farm.order_quotes.iter().all(|e| e.tag == Some(6510)));

    // A second order of the fund: nothing.
    engine.context.order_models.push((id, VFIAX, "FUNDSERV".into()));
    engine.service_option_models(Instant::now());
    assert!(sent(&mut bond_side).is_empty() && sent(&mut farm_side).is_empty());
    assert_eq!(engine.farm.order_quotes.len(), 2);
}
