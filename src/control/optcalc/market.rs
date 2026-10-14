//! Option computations of a market data request (ibx#502), as the
//! reference sends them for an option: tick types 10, 11 and 12 (bid, ask,
//! last) and 13 (model).
//!
//! For an option the reference asks, beside the option's top of book, the
//! top of book of the underlying and its own entries on the volatility
//! exchange: the model greeks (request code 732)
//! and the closing ones (733) of the option, its volatilities (734 the
//! model's, 735 the mid's, 736 the bid's and the ask's, 737 the last's)
//! and the model parameters of the
//! underlying (687), with the market data status of both
//! (398). From them (recorded on the reference 10.51, paper, 12/10/2026,
//! regular hours, an AAPL call):
//!
//! - every implied volatility is the server's, a volatility per trading
//!   day given a year (`* sqrt(252)`): tick 10 the bid's, 11 the ask's, 12
//!   the last's, 13 the model's;
//! - tick 13's price and greeks are the server's model greeks, its
//!   underlying price the one of the model parameters;
//! - ticks 10 to 12 have the greeks of the local model (the one of the
//!   option calculations, `super::model`) at their volatility and the
//!   underlying price, the side's price (a single-precision value), and
//!   the present value of the dividends of the local model, as tick 13.
//!
//! A value not known is `NOT_COMPUTED`.

use std::io::Read;

use super::{ModelInputs, OptionComputation, OptionTerms, NOT_COMPUTED};

/// The request codes of the option model's entries.
pub const GREEKS: i32 = 732;
pub const GREEKS_CLOSE: i32 = 733;
pub const MODEL_VOL: i32 = 734;
pub const MID_VOL: i32 = 735;
pub const BID_ASK_VOL: i32 = 736;
pub const LAST_VOL: i32 = 737;
pub const MODEL_PARAMS: i32 = 687;

/// The code under which the engine queues the option computations of an
/// instrument for its requests (not a request code).
pub const COMPUTATIONS: i32 = -502;
/// The code of computations kept for the requests and not sent to them: a
/// request that joins, or a snapshot at its time limit, gets them (an
/// option whose data is frozen).
pub const COMPUTATIONS_KEPT: i32 = -503;

/// The tick types of the option computations.
pub const BID: i32 = 10;
pub const ASK: i32 = 11;
pub const LAST: i32 = 12;
pub const MODEL: i32 = 13;

/// The delayed tick type of an option computation (80 to 83).
pub fn delayed(tick_type: i32) -> i32 {
    match tick_type {
        BID..=MODEL => tick_type + 70,
        other => other,
    }
}

/// Security types the reference runs its option model for.
pub fn has_model(sec_type: &str) -> bool {
    ["OPT", "FOP", "WAR", "IOPT"].iter().any(|s| s.eq_ignore_ascii_case(sec_type))
}

/// A volatility per trading day given a year (`Volatility.getVol(PerYear)`).
pub fn per_year(daily: f64) -> f64 {
    daily * 252f64.sqrt()
}

fn valid_vol(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4).map(|x| i32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn be_f64(b: &[u8], at: usize) -> Option<f64> {
    b.get(at..at + 8).map(|x| f64::from_be_bytes(x.try_into().unwrap_or([0; 8])))
}

/// A volatility block (734, 735, 737):
/// a count, then the volatility per trading day; valid with a count above 0
/// and a value not below 0 (and above 0, as the record keeps it). None when
/// not valid.
pub fn decode_vol(payload: &[u8]) -> Option<f64> {
    let (n, v) = (be_i32(payload, 0)?, be_f64(payload, 4)?);
    (n > 0 && valid_vol(v)).then_some(v)
}

/// The bid and ask volatility block (736): the bid's, the
/// ask's, then a count; both set when the count is above 0. Each None when
/// not valid.
pub fn decode_bid_ask_vols(payload: &[u8]) -> Option<(Option<f64>, Option<f64>)> {
    let (bid, ask, n) = (be_f64(payload, 0)?, be_f64(payload, 8)?, be_i32(payload, 16)?);
    if n <= 0 {
        return Some((None, None));
    }
    Some((valid_vol(bid).then_some(bid), valid_vol(ask).then_some(ask)))
}

/// The server's model greeks of an option (732).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerGreeks {
    pub price: f64,
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
}

/// A greeks block: a bit set of the values present (`Attributes`), the
/// price, then each value present in the reader's order (delta 16, gamma
/// 17, vega 18, rho 19, theta 20, then values not sent to the client).
/// None when the set has no valid bit (bit 0): the record is cleared.
pub fn decode_greeks(payload: &[u8]) -> Option<ServerGreeks> {
    let bits = be_i32(payload, 0)? as u32;
    if bits & 1 == 0 {
        return None;
    }
    let price = be_f64(payload, 4)?;
    let mut at = 12;
    let mut next = |bit: u32| -> Option<f64> {
        if bits & (1 << bit) == 0 {
            return Some(NOT_COMPUTED);
        }
        let v = be_f64(payload, at)?;
        at += 8;
        Some(v)
    };
    let delta = next(16)?;
    let gamma = next(17)?;
    let vega = next(18)?;
    let _rho = next(19)?;
    let theta = next(20)?;
    Some(ServerGreeks { price, delta, gamma, vega, theta })
}

/// The reference price of the underlying in a model parameters block
/// (687): from the zlib stream after its header, four
/// ints and two doubles, then the symbol (a 32-bit length and its bytes),
/// then the price. Recorded: 339.666015625 and 339.662353515625 on AAPL
/// (12/10/2026), as tick 13 gave them.
pub fn decode_model_price(payload: &[u8]) -> Option<f64> {
    let start = payload.windows(2).position(|w| w == [0x78, 0x9c])?;
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(&payload[start..]).read_to_end(&mut data).ok()?;
    let n = usize::try_from(be_i32(&data, 32)?).ok()?;
    let price = be_f64(&data, 36 + n)?;
    price.is_finite().then_some(price)
}

/// What the option computations of one option are built from.
#[derive(Debug, Clone, Default)]
pub struct ModelState {
    /// Volatilities per trading day: bid, ask, last, mid, model.
    pub bid_vol: Option<f64>,
    pub ask_vol: Option<f64>,
    pub last_vol: Option<f64>,
    pub mid_vol: Option<f64>,
    pub model_vol: Option<f64>,
    pub greeks: Option<ServerGreeks>,
    /// The underlying price of the model parameters.
    pub model_under: Option<f64>,
}

impl ModelState {
    /// The model's volatility: for a stream the model one, else the mid's;
    /// for a snapshot the mid's, else the model one (the reference 10.51,
    /// 12/10/2026: two snapshots with the mid volatility, regular and after
    /// hours; a stream with the model one).
    pub fn model_volatility(&self, snapshot: bool) -> Option<f64> {
        if snapshot { self.mid_vol.or(self.model_vol) } else { self.model_vol.or(self.mid_vol) }
    }
}

/// The underlying price of ticks 10 to 12 from the underlying's quote: the
/// middle of the bid and the ask, or the side nearer to the last trade when
/// that is outside them (the reference 10.51, 12/10/2026: 339.57 with bid
/// 339.55, ask 339.57 and last 339.58, regular hours; 340.58 with bid
/// 340.58, ask 340.68 and last 340.5652, after hours); the last without a
/// bid and an ask. `mid` is the middle on the quote's tick grid.
pub fn under_price(bid: Option<f64>, ask: Option<f64>, last: Option<f64>, mid: impl Fn(f64, f64) -> f64) -> Option<f64> {
    let last = last.filter(|l| *l > 0.0);
    match (bid, ask) {
        (Some(b), Some(a)) if b > 0.0 && a > 0.0 => Some(match last {
            Some(l) if l < b => b,
            Some(l) if l > a => a,
            _ => mid(b, a),
        }),
        _ => last,
    }
}

fn or_unset(v: f64) -> f64 {
    if v.is_finite() && v != f64::MAX { v } else { NOT_COMPUTED }
}

/// A side's price as the computation gives it: a single-precision value
/// (recorded: 14.95 gives 14.949999809265137).
pub fn side_price(p: Option<f64>) -> f64 {
    match p {
        Some(p) if p.is_finite() && p > 0.0 => p as f32 as f64,
        _ => NOT_COMPUTED,
    }
}

/// The present value of the dividends of the local model, when its inputs
/// are known.
pub fn pv_dividend(terms: &OptionTerms, under: f64, inp: Option<&ModelInputs>) -> f64 {
    let Some(inp) = inp else { return NOT_COMPUTED };
    let under = if under.is_finite() { under } else { 0.0 };
    super::build_model(terms, under, inp).map_or(NOT_COMPUTED, |m| or_unset(m.pv_dividend))
}

/// Tick 13: the server's model greeks, the model's volatility, the local
/// present value of the dividends, the model parameters' underlying price.
pub fn model_computation(st: &ModelState, pv: f64, snapshot: bool) -> OptionComputation {
    let g = st.greeks;
    OptionComputation {
        req_id: 0,
        tick_type: MODEL,
        tick_attrib: 0,
        implied_vol: st.model_volatility(snapshot).map_or(NOT_COMPUTED, per_year),
        delta: g.map_or(NOT_COMPUTED, |g| or_unset(g.delta)),
        opt_price: g.map_or(NOT_COMPUTED, |g| or_unset(g.price)),
        pv_dividend: pv,
        gamma: g.map_or(NOT_COMPUTED, |g| or_unset(g.gamma)),
        vega: g.map_or(NOT_COMPUTED, |g| or_unset(g.vega)),
        theta: g.map_or(NOT_COMPUTED, |g| or_unset(g.theta)),
        und_price: st.model_under.map_or(NOT_COMPUTED, or_unset),
    }
}

/// How far a side computation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Before the model's first pass: no volatility, no greeks.
    First,
    /// The volatility only: the model does not compute (frozen data).
    Volatility,
    /// The volatility and the greeks of the local model.
    Computed,
}

/// Tick 10, 11 or 12: the side's price, its volatility from `Form::Volatility`
/// on, and the greeks of the local model at that volatility and `under`
/// with `Form::Computed` when the model inputs are known.
#[allow(clippy::too_many_arguments)]
pub fn side_computation(
    tick_type: i32, terms: &OptionTerms, daily_vol: Option<f64>, price: Option<f64>, under: Option<f64>,
    pv: f64, inp: Option<&ModelInputs>, form: Form,
) -> OptionComputation {
    let mut c = OptionComputation {
        req_id: 0,
        tick_type,
        tick_attrib: 0,
        implied_vol: NOT_COMPUTED,
        delta: NOT_COMPUTED,
        opt_price: side_price(price),
        pv_dividend: pv,
        gamma: NOT_COMPUTED,
        vega: NOT_COMPUTED,
        theta: NOT_COMPUTED,
        und_price: under.map_or(NOT_COMPUTED, or_unset),
    };
    // Frozen, the last's has no volatility either (the reference 10.51
    // after hours, 12/10/2026: bid and ask with theirs, the last without).
    if form == Form::First || (form == Form::Volatility && tick_type == LAST) {
        return c;
    }
    let Some(vol) = daily_vol.map(per_year) else { return c };
    c.implied_vol = vol;
    if form == Form::Computed
        && let (Some(u), Some(inp)) = (under, inp)
        && let Some(a) = super::price_answer(0, terms, vol, u, inp)
    {
        c.delta = a.delta;
        c.gamma = a.gamma;
        c.vega = a.vega;
        c.theta = a.theta;
    }
    c
}

/// Whether a computation is one a snapshot takes (`optionmodel.al.a(int,
/// TopView,Set,Set)`): its volatility, or one of its greeks, is known.
pub fn valid(c: &OptionComputation) -> bool {
    [c.implied_vol, c.delta, c.gamma, c.vega, c.theta].iter().any(|v| *v != NOT_COMPUTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // The blocks of the AAPL call of December 2026, strike 340, recorded on
    // the reference 10.51 (paper, 12/10/2026, regular hours), and the
    // values its ticks gave.
    #[test]
    fn recorded_blocks() {
        assert_eq!(decode_vol(&hex("000000013f8f340f87946520")).map(per_year), Some(0.24186426510722747));
        assert_eq!(decode_vol(&hex("000000013f8f35e8fb9bbef6")).map(per_year), Some(0.24192026245757572));
        assert_eq!(decode_vol(&hex("000000007ff8000000000000")), None, "count 0");
        let (bid, ask) = decode_bid_ask_vols(&hex("3f8ee7d8cc08fc723f8f77504476a90700000001")).unwrap();
        assert_eq!((bid.map(per_year), ask.map(per_year)), (Some(0.2395566468611969), Some(0.24390056272714186)));
        // A deep call: no bid volatility (0).
        let (bid, ask) = decode_bid_ask_vols(&hex("00000000000000003fb786a90bc2b18d00000001")).unwrap();
        assert_eq!((bid, ask.map(per_year)), (None, Some(1.4588435192210518)));
        let g = decode_greeks(&hex("7eb72001402e2a60df0b6f5a3fe170b79a1dcaf63f8734a9ae0b6a653fe26ab5454adc50bfbfdc91228f64804050c63518a6dfc43ff02101a14120a940753ab5e00000003f89ed347d174b634050c63518a6dfc43f1f5b52457e4a1229e9b975f6a8ee9180000000000000004029d8ef20e594ba")).unwrap();
        assert_eq!(g, ServerGreeks {
            price: 15.082770319124098, delta: 0.5450094232640754, gamma: 0.011330915092391433,
            vega: 0.5755258897178326, theta: -0.1244593343138991,
        });
        assert_eq!(decode_greeks(&hex("00000000402e2a60df0b6f5a")), None, "not valid");
        assert_eq!(side_price(Some(14.95)), 14.949999809265137);
        assert_eq!((delayed(10), delayed(13), delayed(1)), (80, 83, 1));
    }

    #[test]
    fn model_price_from_the_parameters() {
        // The model parameters header as recorded (zlib of the first bytes).
        let raw = hex("0000000400000002000005870000000140590000000000003ff0000000000000000000044141504c40753a9900000000");
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &raw).unwrap();
        let mut payload = hex("000004020000060c");
        payload.extend(z.finish().unwrap());
        assert_eq!(decode_model_price(&payload), Some(339.662353515625));
        assert_eq!(decode_model_price(&hex("00000001")), None);
    }

    #[test]
    fn computations_without_inputs() {
        let terms = OptionTerms {
            con_id: 692808239, sec_type: "OPT".into(), currency: "USD".into(), right: Some(super::super::Right::Call),
            style: super::super::Style::American, strike: 340.0, last_trade_date: "20261218".into(),
            last_trade_time: String::new(), time_zone: String::new(), under_con_id: 265598, under_sec_type: "STK".into(),
        };
        let c = side_computation(BID, &terms, Some(0.0150906), Some(14.95), Some(339.57), 0.269, None, Form::First);
        assert_eq!((c.implied_vol, c.delta, c.opt_price, c.pv_dividend, c.und_price), (NOT_COMPUTED, NOT_COMPUTED, 14.949999809265137, 0.269, 339.57));
        assert!(!valid(&c));
        let c = side_computation(BID, &terms, Some(0.01509065029779164), Some(14.95), Some(339.57), 0.269, None, Form::Volatility);
        assert_eq!((c.implied_vol, c.delta), (0.2395566468611969, NOT_COMPUTED));
        assert!(valid(&c));
        let st = ModelState { model_vol: None, mid_vol: Some(0.015315274452962482), model_under: Some(339.662353515625), ..Default::default() };
        let m = model_computation(&st, NOT_COMPUTED, false);
        assert_eq!((m.tick_type, m.implied_vol, m.opt_price, m.und_price), (13, 0.24312244477943712, NOT_COMPUTED, 339.662353515625));
        // A snapshot takes the mid volatility, a stream the model one.
        let st = ModelState { model_vol: Some(0.015236016583502876), ..st };
        assert_eq!(model_computation(&st, NOT_COMPUTED, true).implied_vol, 0.24312244477943712);
        assert_eq!(model_computation(&st, NOT_COMPUTED, false).implied_vol, 0.24186426510722747);
    }

    #[test]
    fn underlying_price_of_the_sides() {
        let mid = |b: f64, a: f64| ((b * 100.0).round() + (a * 100.0).round()) / 200.0;
        assert_eq!(under_price(Some(339.55), Some(339.57), Some(339.58), mid), Some(339.57));
        assert_eq!(under_price(Some(340.58), Some(340.68), Some(340.5652), mid), Some(340.58));
        assert_eq!(under_price(Some(339.56), Some(339.62), Some(339.58), mid), Some(339.59));
        assert_eq!(under_price(None, Some(339.62), Some(339.58), mid), Some(339.58));
        assert_eq!(under_price(None, None, None, mid), None);
    }
}
