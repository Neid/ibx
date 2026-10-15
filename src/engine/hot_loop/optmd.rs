//! The option model of an option's market data (ibx#502), as the
//! reference runs it beside the option's top of book (recorded on the
//! reference 10.51, paper, 12/10/2026, regular hours, an AAPL call):
//!
//! - its entries: the top of book of the underlying, without the API marks;
//!   the model parameters of the underlying (687) and the model greeks of
//!   the option (732, 733) on the volatility exchange; once the model
//!   parameters came, the market data status of the underlying (398), then
//!   the status of the option with its volatilities (734 to 737). A plain
//!   snapshot also gets a streaming pair of the option, as the model's own.
//!   When the model parameters of the underlying are known already, the
//!   option's entries go at once, then the parameters and the status of
//!   the underlying (the snapshot of the recording);
//! - its cancels: for a stream, the underlying's status before the
//!   option's top of book, then the underlying's pair, the option's
//!   entries in descending code order with its status last, the model
//!   parameters; for a snapshot, the greeks, the underlying's status and
//!   parameters, the model's option pair, the underlying's pair, the
//!   volatilities and the option's status;
//! - its computations (see `control::optcalc::market`): a first pass about
//!   a second after the model greeks came, with tick 13 and ticks 10 to 12
//!   without volatility or greeks (the reference's model not computed
//!   yet), then a pass a second later and every two seconds after it, each
//!   sending the ticks whose values changed.
//!
//! The underlying price of ticks 10 to 12 is the middle of the underlying's
//! bid and ask (its last without them): the reference's own differs from
//! it by up to half a cent at times (see the issue tracker).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::bridge::SharedState;
use crate::control::optcalc::{market, OptionComputation, OptionTerms};
use crate::engine::context::Context;
use crate::protocol::fix;
use crate::protocol::tick_decoder;
use crate::types::{InstrumentId, PRICE_SCALE};

use super::pool::FarmId;

/// The first pass after the model greeks came, then the second after it.
const FIRST_PASS: Duration = Duration::from_millis(1000);
/// The passes after the second.
const LATER_PASS: Duration = Duration::from_millis(2000);
/// The pass after the requests moved onto the frozen slot (ibx#494).
const SWITCH_PASS: Duration = Duration::from_millis(250);
/// The first pass of a model that starts from the values kept from an
/// earlier request of the option, after the request's first data (ibx#646).
const KEPT_PASS: Duration = Duration::from_millis(150);
/// How often a model that starts from kept values looks for its first data.
const KEPT_POLL: Duration = Duration::from_millis(20);
/// How long a model waits for its definitions before it is dropped.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long a model stopped by a lost farm waits for it.
const RESTART_WAIT: Duration = Duration::from_secs(120);

/// The option's frozen pair, its bid and ask in frozen mode, asked when the
/// model's status says the option is frozen, for a request in real-time
/// mode too; then one more entry of the option (ibx#647, ibx#494; the
/// reference 10.51, 14 requests on 12/10, 13/10 and 14/10/2026, after hours
/// and before the open). Kept apart from the snapshot's own pair by their
/// sign; the wire has 442 and 443.
const FROZEN_BID: i32 = -442;
const FROZEN_ASK: i32 = -443;
const AFTER_FROZEN: i32 = 317;

/// One entry of a model on the wire.
#[derive(Debug, Clone)]
pub(crate) struct ModelEntry {
    pub(crate) farm_req: u32,
    pub(crate) farm: FarmId,
    pub(crate) con_id: String,
    pub(crate) exchange: String,
    pub(crate) sec_type: String,
    /// Request code: 442 / 443 for a top of book pair, else the tick's;
    /// negative for the frozen pair.
    pub(crate) code: i32,
    /// Of the underlying.
    pub(crate) under: bool,
    pub(crate) tag: Option<u32>,
    pub(crate) min_tick: f64,
    /// The size increment of its acknowledgement, when it gave one.
    pub(crate) size_tick: Option<f64>,
}

/// The option model of one option instrument.
#[derive(Debug, Clone)]
pub(crate) struct OptionModel {
    pub(crate) instrument: InstrumentId,
    pub(crate) snapshot: bool,
    pub(crate) terms: OptionTerms,
    opt_farm: FarmId,
    under_farm: FarmId,
    pub(crate) entries: Vec<ModelEntry>,
    /// The status entries and the volatilities went out.
    vols_sent: bool,
    under_bid: Option<f64>,
    under_ask: Option<f64>,
    under_last: Option<f64>,
    /// The underlying's bid, ask, bid size and ask size as the farm sent
    /// them, and its auto-execution bits: kept for the underlying's next
    /// request when the model stops (ibx#508).
    under_wire: [Option<i64>; 4],
    under_auto: Option<i64>,
    /// The underlying's trade tag: (farm, tag, price tick).
    under_trade: Option<(FarmId, u32, f64)>,
    pub(crate) state: market::ModelState,
    next_pass: Option<Instant>,
    passes: u32,
    sent: [Option<OptionComputation>; 4],
    /// The option's market data status says frozen (1): its model does not
    /// compute.
    frozen: bool,
    /// The frozen pair went out.
    frozen_asked: bool,
    /// The frozen pair brought a trade, for the streams of the option.
    again: bool,
    /// Started from the values kept from an earlier request of the option
    /// (ibx#646): its first pass is computed in full.
    seeded: bool,
    /// Seeded, its first pass waits for the request's first data.
    kept_wait: bool,
    /// The price tick of the underlying's kept quote.
    kept_tick: f64,
}

/// What a stopped model leaves for the next request of its option
/// (ibx#646): the server's model values and the underlying's quote.
#[derive(Debug, Clone)]
struct KeptModel {
    state: market::ModelState,
    under: [Option<f64>; 3],
    under_tick: f64,
}

impl OptionModel {
    /// The underlying price of the computations (`market::under_price`).
    pub(crate) fn under_price(&self) -> Option<f64> {
        let tick = self.entry(442, true).map_or(0.0, |e| e.min_tick);
        // Before its own acknowledgement, the tick of the kept quote.
        let tick = if tick > 0.0 { tick } else { self.kept_tick };
        market::under_price(self.under_bid, self.under_ask, self.under_last, |b, a| {
            // On the quote's tick grid, in whole ticks: 339.56 and 339.61
            // give 339.585, as the reference's, not a neighbour.
            let per_unit = if tick > 0.0 { (1.0 / tick).round() } else { 0.0 };
            if per_unit >= 1.0 && (per_unit * tick - 1.0).abs() < 1e-9 {
                ((b * per_unit).round() + (a * per_unit).round()) / (2.0 * per_unit)
            } else {
                (b + a) / 2.0
            }
        })
    }

    fn entry(&self, code: i32, under: bool) -> Option<&ModelEntry> {
        self.entries.iter().find(|e| e.code == code && e.under == under)
    }
}

/// A model waiting for its definitions (the option's terms, the
/// underlying's aggregate group).
#[derive(Debug, Clone)]
pub(crate) struct Starting {
    pub(crate) instrument: InstrumentId,
    pub(crate) con_id: i64,
    pub(crate) snapshot: bool,
    pub(crate) opt_farm: FarmId,
    pub(crate) deadline: Instant,
}

/// The option models of the market data requests.
#[derive(Debug, Default)]
pub(crate) struct OptMd {
    pub(crate) models: Vec<OptionModel>,
    pub(crate) starting: Vec<Starting>,
    /// The model parameters' price of each underlying, once they came.
    params: HashMap<i64, f64>,
    /// Messages made away from the loop's send points, sent at its next
    /// service.
    pub(crate) outbox: Vec<(FarmId, Vec<(u32, String)>)>,
    /// The logon features that change the entries (ibx#648).
    pub(crate) entry_marks: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// The option whose entry after the frozen pair the models of an
    /// underlying ask: the first that asked it in the session (ibx#647; the
    /// reference 10.51 after hours, 13/10 and 14/10/2026: six requests of
    /// the AAPL 300 call asked it on the 340 call, asked first).
    after_frozen_option: HashMap<i64, i64>,
    /// The values of the stopped models, by the option's conId (ibx#646).
    kept: HashMap<i64, KeptModel>,
}

/// A price of `magnitude` ticks: divided by the ticks per unit when the
/// tick is one over a whole number, so that 33956 ticks of 0.01 are
/// 339.56 and not a neighbour of it.
fn scaled(magnitude: i64, tick: f64) -> f64 {
    let per_unit = (1.0 / tick).round();
    if per_unit >= 1.0 && (per_unit * tick - 1.0).abs() < 1e-9 {
        magnitude as f64 / per_unit
    } else {
        magnitude as f64 * tick
    }
}

/// Market data messages, each with its farm.
pub(crate) type Messages = Vec<(FarmId, Vec<(u32, String)>)>;

fn head(action: &str, n: usize) -> Vec<(u32, String)> {
    vec![
        (fix::TAG_MSG_TYPE, fix::MSG_MARKET_DATA_REQ.to_string()),
        (fix::TAG_SENDING_TIME, crate::config::chrono_free_timestamp().to_string()),
        (263, action.to_string()),
        (146, n.to_string()),
    ]
}

/// A message of entries, as the reference writes the model's: no API
/// source; the frozen mode on the frozen pair; the API mark only on the
/// option's pairs; the entry mark on every pair (ibx#648).
fn message(action: &str, entries: &[&ModelEntry], marks: u8) -> Vec<(u32, String)> {
    let mut msg = head(action, entries.len());
    for e in entries {
        let code = e.code.abs();
        msg.extend([
            (262, e.farm_req.to_string()),
            (6008, e.con_id.clone()),
            (207, e.exchange.clone()),
            (167, e.sec_type.clone()),
            (264, code.to_string()),
        ]);
        if e.code < 0 {
            msg.push((9887, crate::types::MarketDataModes::entry_mode(true, false).to_string()));
        }
        if !e.under && matches!(code, 442 | 443) {
            msg.push((9830, "1".into()));
        }
        if matches!(code, 442 | 443) {
            if let Some(mark) = super::farm::entry_mark(&code.to_string(), marks, false) {
                msg.push((9839, mark.into()));
            }
        }
    }
    msg
}

impl OptMd {
    pub(crate) fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub(crate) fn has(&self, instrument: InstrumentId) -> bool {
        self.models.iter().any(|m| m.instrument == instrument) || self.starting.iter().any(|s| s.instrument == instrument)
    }

    /// Start the model of `instrument` (its top of book went out on
    /// `opt_farm`): its entries, and their messages in the order they go
    /// out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch(
        &mut self, instrument: InstrumentId, con_id: i64, snapshot: bool, terms: OptionTerms, opt_farm: FarmId,
        under_farm: FarmId, under_exchange: &str, next_id: &mut u32,
    ) -> Vec<(FarmId, Vec<(u32, String)>)> {
        let opt_sec = super::farm::fix_sec_type(&terms.sec_type).to_string();
        let under_type = if terms.under_sec_type.is_empty() { "STK" } else { terms.under_sec_type.as_str() };
        let under_sec = super::farm::fix_sec_type(under_type).to_string();
        let mut entries: Vec<ModelEntry> = Vec::new();
        let add = |entries: &mut Vec<ModelEntry>, code: i32, under: bool, exchange: &str| {
            entries.push(ModelEntry {
                farm_req: 0, farm: if under { under_farm } else { opt_farm },
                con_id: if under { terms.under_con_id.to_string() } else { con_id.to_string() },
                exchange: exchange.to_string(), sec_type: if under { under_sec.clone() } else { opt_sec.clone() },
                code, under, tag: None, min_tick: 0.0, size_tick: None,
            });
        };
        if snapshot {
            add(&mut entries, 442, false, "BEST");
            add(&mut entries, 443, false, "BEST");
        }
        add(&mut entries, 442, true, under_exchange);
        add(&mut entries, 443, true, under_exchange);
        add(&mut entries, market::MODEL_PARAMS, true, "IBVOL");
        add(&mut entries, super::farm::MARKET_DATA_STATUS, true, "BEST");
        add(&mut entries, super::farm::MARKET_DATA_STATUS, false, "BEST");
        for code in [market::GREEKS, market::GREEKS_CLOSE, market::MODEL_VOL, market::MID_VOL, market::BID_ASK_VOL, market::LAST_VOL] {
            add(&mut entries, code, false, "IBVOL");
        }
        for code in [FROZEN_BID, FROZEN_ASK, AFTER_FROZEN] {
            add(&mut entries, code, false, "BEST");
        }
        let under_con_id = terms.under_con_id;
        let known = self.params.get(&under_con_id).copied();
        let mut model = OptionModel {
            instrument, snapshot, terms, opt_farm, under_farm, entries, vols_sent: false,
            under_bid: None, under_ask: None, under_last: None, under_wire: [None; 4], under_auto: None, under_trade: None,
            state: market::ModelState::default(), next_pass: None, passes: 0, sent: [None; 4], frozen: false,
            frozen_asked: false, again: false, seeded: false, kept_wait: false, kept_tick: 0.0,
        };
        // An option whose model ran earlier in the session: the new model
        // starts from the values kept, and computes with them a moment
        // after the request's first data, before its own values come (the
        // reference 10.51, paper, 14/10/2026 regular hours: a snapshot after
        // a stream and a stream after a snapshot of the same AAPL call,
        // computations 0.15 s after the first data, with greeks; after
        // hours on 13/10/2026 the same). A new model's first pass is
        // seconds later and has no greeks.
        if let Some(k) = self.kept.get(&con_id) {
            model.state = k.state.clone();
            [model.under_bid, model.under_ask, model.under_last] = k.under;
            model.kept_tick = k.under_tick;
            model.seeded = true;
            model.kept_wait = true;
        }
        let mut groups: Vec<Vec<(i32, bool)>> = Vec::new();
        if snapshot {
            groups.push(vec![(442, false), (443, false)]);
        }
        groups.push(vec![(442, true), (443, true)]);
        if let Some(price) = known {
            // The parameters of the underlying are known: the option's
            // entries at once, then the parameters and the status.
            model.state.model_under = Some(price);
            model.vols_sent = true;
            groups.push(vec![
                (super::farm::MARKET_DATA_STATUS, false), (market::GREEKS, false), (market::GREEKS_CLOSE, false),
                (market::MODEL_VOL, false), (market::MID_VOL, false), (market::BID_ASK_VOL, false), (market::LAST_VOL, false),
            ]);
            groups.push(vec![(market::MODEL_PARAMS, true), (super::farm::MARKET_DATA_STATUS, true)]);
        } else {
            groups.push(vec![(market::MODEL_PARAMS, true)]);
            groups.push(vec![(market::GREEKS, false), (market::GREEKS_CLOSE, false)]);
        }
        let out = Self::send_groups(&mut model, &groups, next_id, self.entry_marks.load(std::sync::atomic::Ordering::Relaxed));
        log::info!("Option model of instrument {} (con_id {}): started on farms {} and {}{}",
            instrument, con_id, opt_farm, under_farm, if snapshot { ", for a snapshot" } else { "" });
        self.models.push(model);
        out
    }

    /// Give request ids to the entries of each group and build its
    /// message.
    fn send_groups(model: &mut OptionModel, groups: &[Vec<(i32, bool)>], next_id: &mut u32, marks: u8) -> Vec<(FarmId, Vec<(u32, String)>)> {
        let mut out = Vec::new();
        for g in groups {
            let mut idx = Vec::new();
            for &(code, under) in g {
                if let Some(i) = model.entries.iter().position(|e| e.code == code && e.under == under && e.farm_req == 0) {
                    model.entries[i].farm_req = *next_id;
                    *next_id += 1;
                    idx.push(i);
                }
            }
            if idx.is_empty() {
                continue;
            }
            let farm = model.entries[idx[0]].farm;
            let es: Vec<&ModelEntry> = idx.iter().map(|&i| &model.entries[i]).collect();
            out.push((farm, message("1", &es, marks)));
        }
        out
    }

    /// The cancels of groups of sent entries, the entries forgotten.
    fn cancel_groups(model: &OptionModel, groups: &[Vec<(i32, bool)>], marks: u8) -> Vec<(FarmId, Vec<(u32, String)>)> {
        let mut out = Vec::new();
        for g in groups {
            let es: Vec<&ModelEntry> = g.iter()
                .filter_map(|&(code, under)| model.entry(code, under).filter(|e| e.farm_req != 0))
                .collect();
            if let Some(first) = es.first() {
                out.push((first.farm, message("2", &es, marks)));
            }
        }
        out
    }

    /// The model of an instrument stops: the cancels to send before its
    /// top of book's, and those after it.
    /// The underlying's bid and ask of the model of `instrument`, as the
    /// farm sent them, for the underlying's next request (ibx#508): its
    /// conId, the four values (bid, ask, bid size, ask size), the price
    /// tick and size increment of its book entry, its auto-execution bits.
    pub(crate) fn under_book(&self, instrument: InstrumentId) -> Option<(i64, [Option<i64>; 4], f64, Option<f64>, Option<i64>)> {
        let m = self.models.iter().find(|m| m.instrument == instrument)?;
        let e = m.entry(442, true).filter(|e| e.tag.is_some())?;
        Some((m.terms.under_con_id, m.under_wire, e.min_tick, e.size_tick, m.under_auto))
    }

    /// The cancels, in one of the reference's two orders (10.51, regular
    /// hours 12/10/2026 and after hours 13/10/2026):
    /// - a snapshot's: the greeks and the entry after the frozen pair, the
    ///   underlying's status and parameters; then the frozen pair, the
    ///   snapshot's own pair, the underlying's pair, the volatilities and
    ///   the option's status;
    /// - a stream's: the underlying's status; then the frozen pair, the
    ///   option's top of book, the underlying's pair, the option's entries
    ///   in descending code order with its status and the entry after the
    ///   frozen pair last, the parameters.
    ///
    /// After hours two of four requests took the other order (see the
    /// issue tracker). The first part leaves in a pass of its own, the
    /// rest about 6 ms later. Returns the first part, the cancels before
    /// the option's top of book, and those after it.
    pub(crate) fn stop(&mut self, instrument: InstrumentId) -> (Messages, Messages, Messages) {
        self.starting.retain(|s| s.instrument != instrument);
        let Some(pos) = self.models.iter().position(|m| m.instrument == instrument) else {
            return (Vec::new(), Vec::new(), Vec::new());
        };
        let m = self.models.remove(pos);
        if m.state.greeks.is_some() {
            let under_tick = m.entry(442, true).map_or(0.0, |e| e.min_tick);
            self.kept.insert(m.terms.con_id, KeptModel {
                state: m.state.clone(), under: [m.under_bid, m.under_ask, m.under_last], under_tick,
            });
        }
        let marks = self.entry_marks.load(std::sync::atomic::Ordering::Relaxed);
        let status = super::farm::MARKET_DATA_STATUS;
        let vols_desc = vec![
            (market::LAST_VOL, false), (market::BID_ASK_VOL, false), (market::MID_VOL, false), (market::MODEL_VOL, false),
        ];
        let frozen = vec![(FROZEN_BID, false), (FROZEN_ASK, false)];
        // The entry after the frozen pair on another option goes alone,
        // last.
        let own = m.terms.con_id.to_string();
        let foreign = m.entry(AFTER_FROZEN, false).is_some_and(|e| e.con_id != own);
        let after: Vec<(i32, bool)> = if foreign { Vec::new() } else { vec![(AFTER_FROZEN, false)] };
        let last_alone = if foreign { vec![(AFTER_FROZEN, false)] } else { Vec::new() };
        if m.snapshot {
            let mut greeks = vec![(market::GREEKS_CLOSE, false), (market::GREEKS, false)];
            greeks.extend(&after);
            let first = Self::cancel_groups(&m, &[greeks, vec![(status, true), (market::MODEL_PARAMS, true)]], marks);
            let mut last = vols_desc;
            last.push((status, false));
            let pre = Self::cancel_groups(&m, &[frozen, vec![(442, false), (443, false)], vec![(442, true), (443, true)], last, last_alone], marks);
            (first, pre, Vec::new())
        } else {
            let first = Self::cancel_groups(&m, &[vec![(status, true)]], marks);
            let pre = Self::cancel_groups(&m, &[frozen], marks);
            let mut opt = vols_desc;
            opt.extend([(market::GREEKS_CLOSE, false), (market::GREEKS, false), (status, false)]);
            opt.extend(&after);
            let post = Self::cancel_groups(&m, &[vec![(442, true), (443, true)], opt, vec![(market::MODEL_PARAMS, true)], last_alone], marks);
            (first, pre, post)
        }
    }

    /// A farm's connection was lost: the models with entries on it stop,
    /// their entries on other farms cancelled; they start again when
    /// their top of book is sent again. The cancels.
    pub(crate) fn farm_lost(&mut self, farm: FarmId) -> Vec<(FarmId, Vec<(u32, String)>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.models.len() {
            if self.models[i].opt_farm != farm && self.models[i].under_farm != farm {
                i += 1;
                continue;
            }
            let m = self.models.remove(i);
            let live: Vec<&ModelEntry> = m.entries.iter().filter(|e| e.farm != farm && e.farm_req != 0).collect();
            if let Some(first) = live.first() {
                out.push((first.farm, message("2", &live, self.entry_marks.load(std::sync::atomic::Ordering::Relaxed))));
            }
            log::info!("Option model of instrument {}: farm {} lost, stopped until it is back", m.instrument, farm);
            self.starting.push(Starting {
                instrument: m.instrument, con_id: m.terms.con_id, snapshot: m.snapshot, opt_farm: m.opt_farm,
                deadline: Instant::now() + RESTART_WAIT,
            });
        }
        out
    }

    /// An acknowledgement of a model entry: the instrument and the entry's
    /// code, None when it is not one.
    pub(crate) fn ack(&mut self, farm: FarmId, req_id: u32, tag: u32, min_tick: f64, size_tick: Option<f64>) -> Option<(InstrumentId, i32, bool)> {
        for m in &mut self.models {
            if let Some(e) = m.entries.iter_mut().find(|e| e.farm == farm && e.farm_req == req_id) {
                e.tag = Some(tag);
                e.min_tick = min_tick;
                e.size_tick = size_tick;
                log::info!("Option model entry {} ack: server_tag {} -> instrument {}", e.code, tag, m.instrument);
                return Some((m.instrument, e.code, e.under));
            }
        }
        None
    }

    /// A model entry by its request number on `farm`.
    pub(crate) fn entry(&self, farm: FarmId, req_id: u32) -> Option<&ModelEntry> {
        self.models.iter().flat_map(|m| &m.entries).find(|e| e.farm == farm && e.farm_req == req_id)
    }

    /// The code of a server tag of a model entry on `farm` (a generic tick
    /// block), None when it is none.
    pub(crate) fn code_of(&self, farm: FarmId, tag: u32) -> Option<i32> {
        self.models.iter().flat_map(|m| &m.entries)
            .find(|e| e.farm == farm && e.tag == Some(tag) && !matches!(e.code.abs(), 442 | 443))
            .map(|e| e.code)
    }

    /// The trade tag of a contract: the models of that underlying read its
    /// last trades there.
    pub(crate) fn ticker_setup(&mut self, farm: FarmId, con_id: i64, tag: u32, min_tick: f64) {
        for m in self.models.iter_mut().filter(|m| m.terms.under_con_id == con_id) {
            m.under_trade = Some((farm, tag, min_tick));
        }
    }

    /// A tick of a top of book message: the underlying's bid, ask and last.
    #[inline]
    pub(crate) fn tick(&mut self, farm: FarmId, t: &tick_decoder::RawTick) {
        use tick_decoder as td;
        // A trade on the frozen pair: the frozen view of the option repeats
        // its last trade (ibx#647).
        if t.tick_type == td::O_LAST_SIZE {
            for m in self.models.iter_mut().filter(|m| m.frozen_asked) {
                if m.entries.iter().any(|e| e.code == FROZEN_BID && e.farm == farm && e.tag == Some(t.server_tag)) {
                    m.again = true;
                }
            }
            return;
        }
        if !matches!(t.tick_type, td::O_BID_PRICE | td::O_ASK_PRICE | td::O_LAST_PRICE
            | td::O_BID_SIZE | td::O_ASK_SIZE | td::O_AUTO_EXEC | td::O_ATTRIBUTES)
        {
            return;
        }
        for m in &mut self.models {
            let quote_tick = m.entries.iter()
                .find(|e| e.under && e.code == 442 && e.farm == farm && e.tag == Some(t.server_tag))
                .map(|e| e.min_tick);
            // The underlying's book as the farm sent it (ibx#508).
            if quote_tick.is_some() && !t.stats_block {
                match t.tick_type {
                    td::O_BID_PRICE => m.under_wire[0] = Some(t.magnitude),
                    td::O_ASK_PRICE => m.under_wire[1] = Some(t.magnitude),
                    td::O_BID_SIZE => m.under_wire[2] = Some(t.magnitude),
                    td::O_ASK_SIZE => m.under_wire[3] = Some(t.magnitude),
                    td::O_AUTO_EXEC | td::O_ATTRIBUTES => m.under_auto = Some(t.magnitude),
                    _ => {}
                }
            }
            if !matches!(t.tick_type, td::O_BID_PRICE | td::O_ASK_PRICE | td::O_LAST_PRICE) {
                continue;
            }
            let trade_tick = m.under_trade.filter(|(f, tag, _)| *f == farm && *tag == t.server_tag).map(|(_, _, tick)| tick);
            let Some(price_tick) = quote_tick.or(trade_tick) else { continue };
            let v = scaled(t.magnitude, price_tick);
            match t.tick_type {
                td::O_BID_PRICE => m.under_bid = Some(v),
                td::O_ASK_PRICE => m.under_ask = Some(v),
                _ => m.under_last = Some(v),
            }
        }
    }

    /// A generic tick block of a model entry: its value goes into the
    /// model; the model parameters, the first time, release the status
    /// entries and the volatilities (`next_id` numbers them).
    pub(crate) fn block(&mut self, farm: FarmId, tag: u32, code: i32, payload: &[u8], now: Instant, next_id: &mut u32) {
        let mut released = Vec::new();
        let marks = self.entry_marks.load(std::sync::atomic::Ordering::Relaxed);
        for m in &mut self.models {
            if !m.entries.iter().any(|e| e.farm == farm && e.tag == Some(tag) && e.code == code) {
                continue;
            }
            let st = &mut m.state;
            match code {
                // A snapshot's model passes when its greeks come (the
                // reference's model tick within 0.5 s of a snapshot, regular
                // and after hours, 12/10/2026), a stream's about a second
                // later.
                market::GREEKS => {
                    st.greeks = market::decode_greeks(payload);
                    if m.next_pass.is_none() && !m.kept_wait && st.greeks.is_some() {
                        m.next_pass = Some(if m.snapshot { now } else { now + FIRST_PASS });
                    }
                }
                market::MODEL_VOL => st.model_vol = market::decode_vol(payload),
                market::MID_VOL => st.mid_vol = market::decode_vol(payload),
                market::LAST_VOL => st.last_vol = market::decode_vol(payload),
                market::BID_ASK_VOL => {
                    if let Some((bid, ask)) = market::decode_bid_ask_vols(payload) {
                        st.bid_vol = bid;
                        st.ask_vol = ask;
                    }
                }
                market::MODEL_PARAMS => {
                    if let Some(price) = market::decode_model_price(payload) {
                        st.model_under = Some(price);
                        self.params.insert(m.terms.under_con_id, price);
                        if !m.vols_sent {
                            m.vols_sent = true;
                            let status = super::farm::MARKET_DATA_STATUS;
                            released.extend(Self::send_groups(m, &[
                                vec![(status, true)],
                                vec![(status, false), (market::MODEL_VOL, false), (market::MID_VOL, false),
                                    (market::BID_ASK_VOL, false), (market::LAST_VOL, false)],
                            ], next_id, marks));
                        }
                    }
                }
                _ => {}
            }
        }
        self.outbox.extend(released);
    }

    /// The market data status of a model's option (a 32-bit value; 1:
    /// frozen). The reference 10.51 after hours (12/10/2026): with the
    /// option frozen, a stream got no computation, a snapshot the model's
    /// at once and the sides' at its time limit, with their volatility and
    /// no greeks.
    ///
    /// Frozen, the model asks the option's frozen pair, then the entry
    /// after it, once; not when the requests are in frozen mode, whose own
    /// frozen pair the status of their top of book asks (ibx#447).
    pub(crate) fn status(&mut self, farm: FarmId, tag: u32, payload: &[u8], frozen_mode: bool, next_id: &mut u32) {
        let Some(value) = payload.get(..4).map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]])) else { return };
        let marks = self.entry_marks.load(std::sync::atomic::Ordering::Relaxed);
        let mut asked = Vec::new();
        for m in &mut self.models {
            if m.entries.iter().any(|e| !e.under && e.code == super::farm::MARKET_DATA_STATUS && e.farm == farm && e.tag == Some(tag)) {
                m.frozen = value == 1;
                log::info!("Option model of instrument {}: market data status {}", m.instrument, value);
                if m.frozen && !frozen_mode && !m.frozen_asked {
                    m.frozen_asked = true;
                    let option = *self.after_frozen_option.entry(m.terms.under_con_id).or_insert(m.terms.con_id);
                    if let Some(e) = m.entries.iter_mut().find(|e| e.code == AFTER_FROZEN) {
                        e.con_id = option.to_string();
                    }
                    asked.extend(Self::send_groups(m, &[
                        vec![(FROZEN_BID, false), (FROZEN_ASK, false)],
                        vec![(AFTER_FROZEN, false)],
                    ], next_id, marks));
                }
            }
        }
        self.outbox.extend(asked);
    }

    /// The requests of the model of `instrument` moved onto its frozen
    /// slot: its values go again, from a pass a little later, once the
    /// requests are there (ibx#494; the reference 10.51, paper, 14/10/2026:
    /// tick 13 0.26 s after the switch).
    pub(crate) fn switched(&mut self, instrument: InstrumentId, now: Instant) {
        for m in self.models.iter_mut().filter(|m| m.instrument == instrument) {
            m.sent = [None; 4];
            m.next_pass = Some(now + SWITCH_PASS);
        }
    }

    /// The options whose frozen pair brought a trade since the last call:
    /// their streams give its size and time again (ibx#647).
    pub(crate) fn take_again(&mut self) -> Vec<InstrumentId> {
        self.models.iter_mut().filter_map(|m| std::mem::take(&mut m.again).then_some(m.instrument)).collect()
    }

    /// The models whose pass is due: their computations that changed go to
    /// the requests of their instrument. `inputs` gives the model inputs of
    /// an option's terms (None while not known).
    pub(crate) fn passes(
        &mut self, now: Instant, context: &Context, shared: &SharedState, quote_slot: impl Fn(InstrumentId) -> InstrumentId,
        mut inputs: impl FnMut(&OptionTerms, &mut dyn FnMut(Option<&crate::control::optcalc::ModelInputs>)),
    ) {
        for m in &mut self.models {
            // The side prices from the frozen slot when the contract's
            // requests are on it (ibx#494; the reference 10.51, paper, after
            // hours 14/10/2026: ticks 11 and 12 of a snapshot in frozen mode
            // with the frozen bid and ask).
            let slot = quote_slot(m.instrument);
            if m.kept_wait && m.next_pass.is_none_or(|t| t <= now) {
                let q = context.quote(slot);
                m.next_pass = Some(if q.bid > 0 || q.ask > 0 {
                    m.kept_wait = false;
                    now + KEPT_PASS
                } else {
                    now + KEPT_POLL
                });
                continue;
            }
            if m.next_pass.is_none_or(|t| t > now) {
                continue;
            }
            let q = context.quote(slot);
            let px = |v: i64| (v > 0).then(|| v as f64 / PRICE_SCALE as f64);
            let (bid, ask, last) = (px(q.bid), px(q.ask), px(q.last));
            let under = m.under_price();
            // In frozen mode the frozen data is the requests' own: a
            // snapshot's model computes on it, and a stream gets tick 13
            // (the reference 10.51, paper, 14/10/2026, after hours and
            // before the open: a snapshot ticks 13, 11 and 12 with greeks,
            // two streams tick 13 only; in real-time mode, 12/10 and
            // 13/10/2026, a stream none and a snapshot tick 13).
            let on_frozen_slot = slot != m.instrument;
            let frozen = m.frozen && !(on_frozen_slot && m.snapshot);
            let form = if frozen {
                market::Form::Volatility
            } else if m.passes > 0 || m.seeded {
                market::Form::Computed
            } else {
                market::Form::First
            };
            let snapshot = m.snapshot;
            let mut comps: Vec<OptionComputation> = Vec::with_capacity(4);
            let (terms, st) = (&m.terms, &m.state);
            inputs(terms, &mut |inp| {
                let pv = market::pv_dividend(terms, under.unwrap_or(f64::NAN), inp);
                comps.push(market::model_computation(st, pv, snapshot));
                comps.push(market::side_computation(market::BID, terms, st.bid_vol, bid, under, pv, inp, form));
                comps.push(market::side_computation(market::ASK, terms, st.ask_vol, ask, under, pv, inp, form));
                comps.push(market::side_computation(market::LAST, terms, st.last_vol, last, under, pv, inp, form));
            });
            // Tick 13 first (`MODEL`), then 10, 11, 12. With the option
            // frozen they are kept, not sent, but for a snapshot's model
            // tick.
            let order = [3usize, 0, 1, 2];
            let (mut sent, mut kept) = (Vec::new(), Vec::new());
            // A snapshot's pass from kept values leaves the last's to the
            // next pass, which ends the snapshot (ibx#646; the reference
            // 10.51, paper, regular hours, 14/10 and 15/10/2026: tick 12 0.6
            // and 1.0 s after 13, 10 and 11, then the end).
            let last_later = m.seeded && snapshot && m.passes == 0;
            for (k, c) in comps.into_iter().enumerate() {
                let slot = order[k];
                // Tick 13 with the server's greeks only.
                if c.tick_type == market::MODEL && st.greeks.is_none() {
                    continue;
                }
                if last_later && c.tick_type == market::LAST {
                    continue;
                }
                if m.sent[slot] == Some(c) {
                    continue;
                }
                m.sent[slot] = Some(c);
                let tick = crate::control::generic_values::GenTick::Option(c);
                // On the frozen slot a stream gets tick 13 only (the
                // reference 10.51, paper, 15/10/2026 pre-market, case A).
                let keep = if c.tick_type == market::MODEL {
                    frozen && !(snapshot || on_frozen_slot)
                } else {
                    frozen || (on_frozen_slot && !snapshot)
                };
                if keep {
                    kept.push(tick);
                } else {
                    sent.push(tick);
                }
            }
            m.passes += 1;
            m.next_pass = Some(now + if m.passes == 1 { FIRST_PASS } else { LATER_PASS });
            for (code, ticks) in [(market::COMPUTATIONS, sent), (market::COMPUTATIONS_KEPT, kept)] {
                if !ticks.is_empty() {
                    shared.market.push_generic_ticks(crate::bridge::GenericTicks {
                        at: shared.market.md_events.position(), instrument: m.instrument, code, ticks,
                    });
                }
            }
        }
    }

    /// The models waiting for definitions that waited too long are
    /// dropped.
    pub(crate) fn sweep_starting(&mut self, now: Instant) {
        self.starting.retain(|s| {
            if s.deadline > now { return true; }
            log::warn!("Option model of instrument {} (con_id {}): no definition in time, no option computations", s.instrument, s.con_id);
            false
        });
    }

    pub(crate) fn start_wait() -> Duration {
        START_WAIT
    }

    #[cfg(test)]
    pub(crate) fn force_pass_for_test(&mut self, instrument: InstrumentId, now: Instant) {
        if let Some(m) = self.models.iter_mut().find(|m| m.instrument == instrument) {
            m.next_pass = Some(now);
        }
    }
}
