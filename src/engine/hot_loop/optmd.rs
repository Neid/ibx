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
//!   yet), then the passes of the session's clock, each sending the ticks
//!   whose values changed.
//!
//! The session's clock (ibx#684): after its first pass, every model passes
//! on one clock for the whole session, shared across requests, cancels and
//! contracts, that ticks on the even seconds of the machine's wall clock.
//! Observed in the recordings of 13-16/10/2026 (the reference 10.51,
//! paper, 10 sessions, 5 to 124 stream passes each): the passes 0.00 to
//! 0.26 s after the even second; a request's first pass at once when its
//! data came, off the clock; what it lacked at the next tick. A snapshot
//! thus ends at the first even second after its first data.
//!
//! The underlying price of ticks 10 to 12 is the middle of the underlying's
//! bid and ask (its last without them): the reference's own differs from
//! it by up to half a cent at times (see the issue tracker).
//!
//! An order on an option or a futures option starts a model too (ibx#693),
//! the engine's own: the option's top of book is one of its entries, with
//! no API mark, and its computations go to no one. Observed in the
//! recordings of 13-16/10/2026 (the reference 10.51, paper, 5 option
//! orders and 1 futures option order, the first of their contract in the
//! session): the underlying's pair and model parameters, then the option's
//! pair and greeks, queued for the option's farm while it connects; then
//! the statuses and volatilities as a request's model. It was kept to the
//! end of each recording (13 to 40 s), with no cancel.
//!
//! A futures option order's model has an index part too (ibx#727), on a
//! farm of its own: the index the future's index names as its reference
//! (the ES future's index names the SPX index). Observed on paper,
//! 15/10/2026, before the open (the reference 10.51, an ES put ordered on
//! CME by conId, the first futures option order of the session): once the
//! definitions naming it were read, about 0.5 s after the future's
//! entries, its native top with its bid and ask pair, then its market data
//! status, on its own exchange, all queued while its farm connected; the
//! status said frozen, and 2 ms after it its frozen pair went out, then its
//! entry 613 on the volatility exchange. Kept to the end of the recording,
//! with no cancel.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::bridge::SharedState;
use crate::control::optcalc::{market, OptionComputation, OptionTerms};
use crate::engine::context::Context;
use crate::protocol::fix;
use crate::protocol::tick_decoder;
use crate::types::{InstrumentId, PRICE_SCALE};

use super::pool::FarmId;

/// The first pass of a stream's new model after the model greeks came.
const FIRST_PASS: Duration = Duration::from_millis(1000);
/// The period of the session's clock: its ticks fall on the even seconds
/// of the wall clock (ibx#684).
const CLOCK: Duration = Duration::from_secs(2);
/// The pass after the requests moved onto the frozen slot (ibx#494).
const SWITCH_PASS: Duration = Duration::from_millis(250);
/// The first pass of a model that starts from the values kept from an
/// earlier request of the option, after the request's first data (ibx#646).
const KEPT_PASS: Duration = Duration::from_millis(150);
/// The pass after the own greeks of a model started from kept values.
const OWN_PASS: Duration = Duration::from_millis(100);
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
/// The index part's entry after its frozen pair (ibx#727).
const INDEX_AFTER_FROZEN: i32 = 613;
/// The index part's native top entry (ibx#640, ibx#727).
const INDEX_NATIVE_TOP: i32 = 403;

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
    /// Seeded, its own greeks did not come yet.
    own_wait: bool,
    /// Its own greeks came before its first pass: a second pass right
    /// after it.
    own_pending: bool,
    /// The price tick of the underlying's kept quote.
    kept_tick: f64,
    /// Started by an order on that exchange (ibx#693): the engine's own
    /// model, whose computations go to no one.
    pub(crate) order: Option<String>,
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
    /// Started by an order on that exchange (ibx#693): its option's route
    /// is found once its definition is known.
    pub(crate) order: Option<String>,
}

/// The index part of a futures option order's model (ibx#727): the
/// entries of the index on its farm, the engine's own; their data goes to
/// no one.
#[derive(Debug, Clone)]
pub(crate) struct IndexPart {
    /// The order's option and its future.
    pub(crate) instrument: InstrumentId,
    pub(crate) future: i64,
    /// The index.
    pub(crate) con_id: i64,
    pub(crate) farm: FarmId,
    pub(crate) entries: Vec<ModelEntry>,
    /// The frozen pair went out.
    frozen_asked: bool,
}

/// A futures option order's model waiting for the definitions that name
/// its index (ibx#727): the future's, the index's it names, the index's
/// that one names as its reference.
#[derive(Debug, Clone)]
pub(crate) struct IndexStart {
    pub(crate) instrument: InstrumentId,
    pub(crate) future: i64,
    pub(crate) deadline: Instant,
    /// The last definition asked for it.
    pub(crate) asked: Option<i64>,
}

/// What a futures option order's model asks once its index part knows the
/// index its future names and that index's reference index (ibx#730; the
/// reference 10.51, paper, 15/10/2026 before the open, an ES put ordered on
/// CME): the trading hours of the future's class 2 ms after the reference
/// index's definition; once they came (5 ms), the walks of index lookups
/// and the model's inputs, the rate curve and the dividends of the future's
/// index.
#[derive(Debug, Clone)]
pub(crate) struct IndexFollow {
    pub(crate) instrument: InstrumentId,
    pub(crate) future: i64,
    /// The future's index, whose dividends the model reads.
    pub(crate) index: i64,
    /// The index that one refers to.
    pub(crate) target: i64,
    pub(crate) future_key: String,
    pub(crate) currency: String,
    /// Past it, the inputs are asked without the trading hours.
    pub(crate) deadline: Instant,
}

/// An order's model whose inputs were asked: once they came, the trading
/// hours of the class of the contract whose dividends it reads (ibx#730;
/// the reference 10.51, paper, observed in the recordings of 13-16/10/2026:
/// four first orders of four, three on an AAPL option, the hours of AAPL's
/// class 5 to 13 ms after the last of the two answers, and an ES futures
/// option, those of the ES index's class 6 ms after).
#[derive(Debug, Clone)]
pub(crate) struct InputHours {
    pub(crate) currency: String,
    pub(crate) under_con_id: i64,
    /// Past it, nothing is asked.
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
    /// An instant on an even second of the wall clock, the origin of the
    /// session's clock (ibx#684); set at its first use.
    clock_origin: Option<Instant>,
    /// The index parts of the futures option orders' models, one per index
    /// for the session (ibx#727), and those waiting for their definitions.
    pub(crate) indexes: Vec<IndexPart>,
    pub(crate) index_starts: Vec<IndexStart>,
    /// The futures option order models' steps after their index part's
    /// definitions (ibx#730).
    pub(crate) index_follows: Vec<IndexFollow>,
    pub(crate) input_hours: Vec<InputHours>,
    /// The futures whose steps were taken: once per future in the
    /// session, an index part asked again after a farm loss included.
    pub(crate) followed_futures: std::collections::HashSet<i64>,
}

/// An instant on an even second of the wall clock, before every instant
/// the loop will pass.
fn even_second_origin() -> Instant {
    let now = Instant::now();
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let phase = Duration::from_nanos((wall.as_nanos() % CLOCK.as_nanos()) as u64);
    // Far back, so that the instants of the session all come after it.
    now.checked_sub(phase + CLOCK * 1000).or_else(|| now.checked_sub(phase)).unwrap_or(now)
}

/// The first tick of the session's clock after `now`.
#[inline]
fn tick_after(origin: Instant, now: Instant) -> Instant {
    let period = CLOCK.as_nanos();
    let since = now.saturating_duration_since(origin).as_nanos();
    origin + Duration::from_nanos(((since / period + 1) * period) as u64)
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
/// option's pairs of a request's model (`api`); the entry mark on every
/// pair (ibx#648).
fn message(action: &str, entries: &[&ModelEntry], marks: u8, api: bool) -> Vec<(u32, String)> {
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
        if api && !e.under && matches!(code, 442 | 443) {
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

    /// A model of the contract runs or waits for its definitions.
    pub(crate) fn has_contract(&self, con_id: i64) -> bool {
        self.models.iter().any(|m| m.terms.con_id == con_id) || self.starting.iter().any(|s| s.con_id == con_id)
    }

    /// The model of `instrument` is one an order started (ibx#693).
    pub(crate) fn is_order_model(&self, instrument: InstrumentId) -> bool {
        self.models.iter().any(|m| m.instrument == instrument && m.order.is_some())
            || self.starting.iter().any(|s| s.instrument == instrument && s.order.is_some())
    }

    /// A model has entries on `farm`: the farm stays up for them.
    pub(crate) fn uses_farm(&self, farm: FarmId) -> bool {
        self.models.iter().any(|m| m.opt_farm == farm || m.under_farm == farm)
            || self.indexes.iter().any(|p| p.farm == farm)
    }

    /// A futures option order's model started on the option `instrument`,
    /// whose future is `future`: its index part waits for the definitions
    /// that name the index (ibx#727). Once per future.
    pub(crate) fn note_index_start(&mut self, instrument: InstrumentId, future: i64, now: Instant) {
        if future <= 0 || self.index_starts.iter().any(|s| s.future == future)
            || self.indexes.iter().any(|p| p.future == future)
        {
            return;
        }
        self.index_starts.push(IndexStart { instrument, future, deadline: now + START_WAIT, asked: None });
    }

    /// The index parts waiting too long for their definitions are dropped.
    pub(crate) fn sweep_index_starts(&mut self, now: Instant) {
        self.index_starts.retain(|s| {
            if s.deadline > now { return true; }
            log::warn!("Index part of the model of instrument {} (future {}): no definition in time", s.instrument, s.future);
            false
        });
    }

    /// The index has its part already.
    pub(crate) fn has_index(&self, con_id: i64) -> bool {
        self.indexes.iter().any(|p| p.con_id == con_id)
    }

    /// Start the index part of a futures option order's model (ibx#727),
    /// on `farm`, as the reference asked it (paper, 15/10/2026): the native
    /// top with the bid and ask pair in one message, the market data
    /// status in a second, both on the index's own exchange, with the entry
    /// marks and no API mark. The frozen pair and the entry after it wait
    /// for the status. Their messages.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch_index(
        &mut self, instrument: InstrumentId, future: i64, con_id: i64, exchange: &str, sec_type: &str, farm: FarmId,
        next_id: &mut u32,
    ) -> Messages {
        if self.has_index(con_id) {
            return Vec::new();
        }
        let sec = super::farm::fix_sec_type(sec_type).to_string();
        let entry = |code: i32, exchange: &str| ModelEntry {
            farm_req: 0, farm, con_id: con_id.to_string(), exchange: exchange.to_string(), sec_type: sec.clone(),
            code, under: true, tag: None, min_tick: 0.0, size_tick: None,
        };
        let mut part = IndexPart {
            instrument, future, con_id, farm,
            entries: vec![
                entry(INDEX_NATIVE_TOP, exchange), entry(442, exchange), entry(443, exchange),
                entry(super::farm::MARKET_DATA_STATUS, exchange),
                entry(FROZEN_BID, exchange), entry(FROZEN_ASK, exchange), entry(INDEX_AFTER_FROZEN, "IBVOL"),
            ],
            frozen_asked: false,
        };
        let marks = self.entry_marks.load(std::sync::atomic::Ordering::Relaxed);
        let out = Self::index_groups(&mut part, &[&[INDEX_NATIVE_TOP, 442, 443], &[super::farm::MARKET_DATA_STATUS]], next_id, marks);
        log::info!("Index part of the model of instrument {}: index {} on {} (farm {})", instrument, con_id, exchange, farm);
        self.indexes.push(part);
        out
    }

    /// Give request ids to the index part's entries of each group, and
    /// their messages.
    fn index_groups(part: &mut IndexPart, groups: &[&[i32]], next_id: &mut u32, marks: u8) -> Messages {
        let mut out = Vec::new();
        for g in groups {
            let mut idx = Vec::new();
            for &code in g.iter() {
                if let Some(i) = part.entries.iter().position(|e| e.code == code && e.farm_req == 0) {
                    part.entries[i].farm_req = *next_id;
                    *next_id += 1;
                    idx.push(i);
                }
            }
            if idx.is_empty() {
                continue;
            }
            let es: Vec<&ModelEntry> = idx.iter().map(|&i| &part.entries[i]).collect();
            out.push((part.farm, message("1", &es, marks, false)));
        }
        out
    }

    /// An acknowledgement of an index part's entry (ibx#727): its tag is
    /// kept. False when it is not one.
    pub(crate) fn index_ack(&mut self, farm: FarmId, req_id: u32, tag: u32, min_tick: f64) -> bool {
        for p in &mut self.indexes {
            if let Some(e) = p.entries.iter_mut().find(|e| e.farm == farm && e.farm_req == req_id) {
                e.tag = Some(tag);
                e.min_tick = min_tick;
                log::info!("Index part entry {} ack: server_tag {} -> index {}", e.code, tag, p.con_id);
                return true;
            }
        }
        false
    }

    /// Start the model of `instrument` (its top of book went out on
    /// `opt_farm`): its entries, and their messages in the order they go
    /// out.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch(
        &mut self, instrument: InstrumentId, con_id: i64, snapshot: bool, terms: OptionTerms, opt_farm: FarmId,
        under_farm: FarmId, under_exchange: &str, next_id: &mut u32,
    ) -> Vec<(FarmId, Vec<(u32, String)>)> {
        self.launch_with(instrument, con_id, snapshot, terms, opt_farm, under_farm, under_exchange, None, next_id)
    }

    /// [`launch`](Self::launch), for a request (`order` None) or for an
    /// order on that exchange (ibx#693): the option's own pair on the
    /// order's exchange is one of its entries, asked after the
    /// underlying's parameters (the reference 10.51, observed in the
    /// recordings of 13-16/10/2026), or first when they are known (13/10).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch_with(
        &mut self, instrument: InstrumentId, con_id: i64, snapshot: bool, terms: OptionTerms, opt_farm: FarmId,
        under_farm: FarmId, under_exchange: &str, order: Option<String>, next_id: &mut u32,
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
        if let Some(exchange) = &order {
            let exchange = super::farm::routing_exchange(exchange, &terms.sec_type);
            add(&mut entries, 442, false, exchange);
            add(&mut entries, 443, false, exchange);
        } else if snapshot {
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
            frozen_asked: false, again: false, seeded: false, kept_wait: false, own_wait: false, own_pending: false, kept_tick: 0.0,
            order,
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
            model.own_wait = true;
            log::info!("Option model of instrument {}: starts from the values kept", instrument);
        }
        let mut groups: Vec<Vec<(i32, bool)>> = Vec::new();
        let by_order = model.order.is_some();
        if snapshot || (by_order && known.is_some()) {
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
            if by_order {
                groups.push(vec![(442, false), (443, false)]);
            }
            groups.push(vec![(market::GREEKS, false), (market::GREEKS_CLOSE, false)]);
        }
        let out = Self::send_groups(&mut model, &groups, next_id, self.entry_marks.load(std::sync::atomic::Ordering::Relaxed));
        log::info!("Option model of instrument {} (con_id {}): started on farms {} and {}{}",
            instrument, con_id, opt_farm, under_farm,
            if by_order { ", for an order" } else if snapshot { ", for a snapshot" } else { "" });
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
            out.push((farm, message("1", &es, marks, model.order.is_none())));
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
                out.push((first.farm, message("2", &es, marks, model.order.is_none())));
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
        if m.order.is_some() {
            // An order's model: no cancel of it was observed (it was kept
            // to the end of the recordings); all of its entries, in one
            // pass, the option's pair first.
            let mut opt = vols_desc;
            opt.extend([(market::GREEKS_CLOSE, false), (market::GREEKS, false), (status, false)]);
            opt.extend(&after);
            let all = Self::cancel_groups(&m, &[vec![(442, false), (443, false)], frozen, vec![(status, true)],
                vec![(442, true), (443, true)], opt, vec![(market::MODEL_PARAMS, true)], last_alone], marks);
            return (Vec::new(), all, Vec::new());
        }
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
                out.push((first.farm, message("2", &live, self.entry_marks.load(std::sync::atomic::Ordering::Relaxed), m.order.is_none())));
            }
            log::info!("Option model of instrument {}: farm {} lost, stopped until it is back", m.instrument, farm);
            self.starting.push(Starting {
                instrument: m.instrument, con_id: m.terms.con_id, snapshot: m.snapshot, opt_farm: m.opt_farm,
                deadline: Instant::now() + RESTART_WAIT, order: m.order,
            });
        }
        // An index part on it is asked again once its farm is back (a
        // loss was not observed; ibx#727).
        let mut k = 0;
        while k < self.indexes.len() {
            if self.indexes[k].farm != farm {
                k += 1;
                continue;
            }
            let p = self.indexes.remove(k);
            log::info!("Index part of index {}: farm {} lost, asked again when it is back", p.con_id, farm);
            self.index_starts.push(IndexStart {
                instrument: p.instrument, future: p.future, deadline: Instant::now() + RESTART_WAIT, asked: None,
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
            .chain(self.indexes.iter().flat_map(|p| &p.entries))
            .find(|e| e.farm == farm && e.tag == Some(tag) && !matches!(e.code.abs(), 442 | 443 | INDEX_NATIVE_TOP))
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
            for m in self.models.iter_mut().filter(|m| m.frozen_asked && m.order.is_none()) {
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
                    // A model started from kept values passes again when
                    // its own greeks come (ibx#646; the reference 10.51,
                    // paper, regular hours: 0.1 s after them on 15/10/2026,
                    // 0.26 s after its first pass; in the same millisecond
                    // as it on 14/10; after hours on 13/10/2026 with them).
                    if m.own_wait && st.greeks.is_some() {
                        m.own_wait = false;
                        if !m.snapshot {
                            if m.passes == 0 {
                                m.own_pending = true;
                            } else {
                                m.next_pass = Some(now + OWN_PASS);
                            }
                            log::info!("Option model of instrument {}: its own greeks came, a pass", m.instrument);
                        }
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
        // The index part's status (ibx#727; the reference 10.51, paper,
        // 15/10/2026 before the open): frozen, its frozen pair, then its
        // entry after it, alone, once. Not frozen was not observed: nothing
        // more is asked.
        for p in &mut self.indexes {
            if p.frozen_asked
                || !p.entries.iter().any(|e| e.code == super::farm::MARKET_DATA_STATUS && e.farm == farm && e.tag == Some(tag))
            {
                continue;
            }
            log::info!("Index part of index {}: market data status {}", p.con_id, value);
            if value == 1 {
                p.frozen_asked = true;
                asked.extend(Self::index_groups(p, &[&[FROZEN_BID, FROZEN_ASK], &[INDEX_AFTER_FROZEN]], next_id, marks));
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
        let origin = *self.clock_origin.get_or_insert_with(even_second_origin);
        for m in &mut self.models {
            // An order's model computes for no one (ibx#693).
            if m.order.is_some() {
                continue;
            }
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
            // next pass, the clock's next tick, which ends the snapshot
            // (ibx#646, ibx#684; the reference 10.51, paper, regular hours,
            // observed in the recordings of 13-16/10/2026: tick 12 at the
            // first even second after 13, 10 and 11, then the end, 10 of
            // 10 snapshots).
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
            // The next pass on the session's clock (ibx#684).
            m.next_pass = Some(tick_after(origin, now));
            if std::mem::take(&mut m.own_pending) {
                m.next_pass = Some(now);
            }
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

    /// The session's clock ticks at `origin` and every two seconds from it.
    #[cfg(test)]
    pub(crate) fn set_clock_for_test(&mut self, origin: Instant) {
        self.clock_origin = Some(origin);
    }

    #[cfg(test)]
    pub(crate) fn force_pass_for_test(&mut self, instrument: InstrumentId, now: Instant) {
        if let Some(m) = self.models.iter_mut().find(|m| m.instrument == instrument) {
            m.next_pass = Some(now);
        }
    }
}

/// The session's clock of the computations (ibx#684).
#[cfg(test)]
mod clock_tests {
    use super::*;
    use crate::control::optcalc::{Right, Style};

    const OPT_A: i64 = 692808239;
    const OPT_B: i64 = 692808240;

    fn terms(con_id: i64) -> OptionTerms {
        OptionTerms {
            con_id, sec_type: "OPT".into(), currency: "USD".into(), right: Some(Right::Call), style: Style::American,
            strike: 340.0, last_trade_date: "20261218".into(), last_trade_time: String::new(), time_zone: "US/Eastern".into(),
            under_con_id: 265598, under_sec_type: "STK".into(),
        }
    }

    /// The models' computations queued since the last call, by tick type.
    fn given(shared: &SharedState) -> Vec<(InstrumentId, i32)> {
        shared.market.take_generic_ticks(u64::MAX).into_iter()
            .filter(|g| g.code == market::COMPUTATIONS)
            .flat_map(|g| g.ticks.into_iter().map(move |t| (g.instrument, t)))
            .filter_map(|(i, t)| if let crate::control::generic_values::GenTick::Option(c) = t { Some((i, c.tick_type)) } else { None })
            .collect()
    }

    /// A context with a quote for each option, and an engine of models
    /// whose clock ticks at `base` and every two seconds from it.
    fn setup(base: Instant, options: &[(i64, bool)]) -> (OptMd, Context, SharedState, Vec<InstrumentId>) {
        let mut md = OptMd::default();
        md.set_clock_for_test(base.checked_sub(CLOCK * 10).expect("an instant 20 s back"));
        let mut context = Context::new();
        let shared = SharedState::new();
        let mut next_id = 1;
        let mut ids = Vec::new();
        for &(con_id, snapshot) in options {
            let id = context.market.register(con_id);
            let q = context.market.quote_mut(id);
            q.bid = 1495 * PRICE_SCALE / 100;
            q.ask = 1520 * PRICE_SCALE / 100;
            q.last = 1500 * PRICE_SCALE / 100;
            let _ = md.launch(id, con_id, snapshot, terms(con_id), 2, 0, "BEST", &mut next_id);
            let m = md.models.last_mut().unwrap();
            [m.under_bid, m.under_ask] = [Some(339.56), Some(339.61)];
            ids.push(id);
        }
        (md, context, shared, ids)
    }

    fn run(md: &mut OptMd, context: &Context, shared: &SharedState, now: Instant) {
        md.passes(now, context, shared, |i| i, |_, f| f(None));
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    // The ticks fall on the even seconds of the wall clock: the origin
    // found is one, and a tick is the first after the instant asked.
    #[test]
    fn the_clock_ticks_on_the_even_seconds() {
        let wall = std::time::SystemTime::now();
        let now = Instant::now();
        let origin = even_second_origin();
        let next = tick_after(origin, now);
        let at = wall + (next - now);
        let ms_of = at.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        // Within the few microseconds between the two clocks' reads.
        let phase = ms_of % 2000;
        assert!(phase <= 2 || phase >= 1998, "next tick {} ms past an even second", phase);
        assert!(next > now && next - now <= CLOCK);
        assert_eq!(tick_after(origin, next), next + CLOCK, "a tick asked on a tick: the next one");
        assert_eq!(tick_after(origin, next - ms(1)), next);
    }

    // A snapshot's first pass goes at once when its data came, without the
    // last's computation; the last's at the next even second, which ends
    // the snapshot (the reference 10.51, observed in the recordings of
    // 13-16/10/2026: 10 of 10 snapshots).
    #[test]
    fn a_snapshot_gives_the_last_at_the_next_tick() {
        let base = Instant::now();
        let (mut md, context, shared, ids) = setup(base, &[(OPT_A, true)]);
        let m = &mut md.models[0];
        m.seeded = true;
        // Its first data 0.3 s after a tick: its first pass at once.
        m.next_pass = Some(base + ms(300));
        run(&mut md, &context, &shared, base + ms(299));
        assert!(given(&shared).is_empty());
        run(&mut md, &context, &shared, base + ms(300));
        assert_eq!(given(&shared), [(ids[0], 10), (ids[0], 11)], "no last yet");
        assert_eq!(md.models[0].next_pass, Some(base + CLOCK));
        run(&mut md, &context, &shared, base + ms(1999));
        assert!(given(&shared).is_empty(), "not before the tick");
        run(&mut md, &context, &shared, base + CLOCK);
        assert_eq!(given(&shared), [(ids[0], 12)], "the last at the tick");
        assert_eq!(md.models[0].next_pass, Some(base + CLOCK * 2));
    }

    // Two requests' models pass on the same ticks, whenever their first
    // data came; a model started later, after a cancel, joins them.
    #[test]
    fn two_requests_share_the_ticks() {
        let base = Instant::now();
        let (mut md, context, shared, ids) = setup(base, &[(OPT_A, false), (OPT_B, false)]);
        md.models[0].next_pass = Some(base + ms(150));
        md.models[1].next_pass = Some(base + ms(1700));
        run(&mut md, &context, &shared, base + ms(150));
        assert_eq!(given(&shared).len(), 3, "the first's first pass, at once");
        run(&mut md, &context, &shared, base + ms(1700));
        assert_eq!(given(&shared).len(), 3, "the second's first pass, at once");
        assert_eq!(md.models[0].next_pass, Some(base + CLOCK));
        assert_eq!(md.models[1].next_pass, Some(base + CLOCK));
        for k in 1..4 {
            run(&mut md, &context, &shared, base + CLOCK * k);
            assert_eq!(md.models.iter().map(|m| m.next_pass).collect::<Vec<_>>(), [Some(base + CLOCK * (k + 1)); 2]);
        }
        // The second ends; a request of it again later passes on the
        // same ticks as the first.
        let _ = md.stop(ids[1]);
        let mut next_id = 100;
        let _ = md.launch(ids[1], OPT_B, false, terms(OPT_B), 2, 0, "BEST", &mut next_id);
        md.models[1].next_pass = Some(base + CLOCK * 4 + ms(900));
        run(&mut md, &context, &shared, base + CLOCK * 4 + ms(900));
        assert_eq!(md.models.iter().map(|m| m.next_pass).collect::<Vec<_>>(), [Some(base + CLOCK * 5); 2]);
        // A pass done late stays on the clock.
        run(&mut md, &context, &shared, base + CLOCK * 5 + ms(40));
        assert_eq!(md.models.iter().map(|m| m.next_pass).collect::<Vec<_>>(), [Some(base + CLOCK * 6); 2]);
    }

    // A new model's snapshot: its first pass when the model greeks came,
    // the computed sides at the next tick, which completes it.
    #[test]
    fn a_new_snapshot_completes_at_the_next_tick() {
        let base = Instant::now();
        let (mut md, context, shared, _) = setup(base, &[(OPT_A, true)]);
        md.models[0].next_pass = Some(base + ms(1200));
        run(&mut md, &context, &shared, base + ms(1200));
        assert_eq!(md.models[0].passes, 1);
        assert_eq!(md.models[0].next_pass, Some(base + CLOCK));
        run(&mut md, &context, &shared, base + ms(1990));
        assert_eq!(md.models[0].passes, 1);
        run(&mut md, &context, &shared, base + CLOCK);
        assert_eq!(md.models[0].passes, 2);
    }
}
