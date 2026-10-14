//! The market data callbacks of the API client, step by step (ibx#446).
//!
//! The engine queues each step of each farm message (`md_events`); the
//! dispatch reads them in their order and gives each request of the
//! instrument the callbacks the reference sends for that step, with that
//! step's values, however many messages came since the last dispatch:
//!
//! - a stream sends what changed since it last sent it (a value is sent
//!   when it changes; sizes and volume start empty, so a first 0 is sent; a
//!   price goes out with its size, and a changed size goes out again on its
//!   own), in the fixed order of the reference's sender: bid, ask, last
//!   (each with its size), bid size, ask size, last size, volume, high, low,
//!   close, open, bid and ask exchanges, last time, halted
//!   (the
//!   delayed sender has the volume after the low and no exchanges). The
//!   halted state is sent when its halted or volatility-halted bit changes,
//!   then in each step until the next book update. The last exchange is
//!   never sent;
//! - a plain snapshot sends each tick type once, from the steps that set
//!   it, then its end;
//! - a request that joins a quote with data gets all of it in one step
//!   first, at its place in the queue.
//!
//! A request has its place in the queue: it gets the steps written after
//! it was made (a request that joins once the engine found its contract,
//! from that moment), and, once cancelled, still the steps written before
//! the cancel, as the reference had sent them to the API socket already.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::api::types::PRICE_SCALE_F;
use crate::bridge::SharedState;
use crate::control::optcalc::{market, OptionComputation};
use crate::control::snapshot::PlainSnapshot;
use crate::md_events::{field, MdEvent, MdStep};
use crate::types::*;

use super::*;

/// One market data callback of the dispatch, in the order they go out.
#[derive(Debug, Clone, PartialEq)]
pub enum MdOut {
    Tick(i64, MdTick),
    /// `market_data_type`, before the first tick of a request.
    MarketDataType(i64, i32),
    /// `tick_snapshot_end`; the request is over.
    SnapshotEnd(i64),
}

/// What a request sent last and where it reads the queue (ibx#446).
#[derive(Debug, Default, Clone)]
pub struct StreamState {
    fields: [i64; field::COUNT],
    /// The sizes and volume sent once (`SizeKind` bits).
    sizes_sent: u8,
    /// The halted and volatility-halted bits of the last trade status.
    halted_bits: u8,
    /// The halted state goes out in each step until the next book update.
    halted_pending: bool,
    /// Steps before this queue position are not the request's.
    start: u64,
    /// The request joined a quote: its first step sends all of it, once
    /// its place in the queue is reached.
    waiting_join: bool,
    /// The snapshot ended: nothing more.
    ended: bool,
    /// Its market data type was looked at, at its first tick.
    pub(crate) mdt_done: bool,
    /// Delayed data: delayed tick types, the delayed sender's order.
    pub(crate) delayed: bool,
    /// The bid and ask auto-execution comes from the farm.
    farm_auto: bool,
    /// `mdoff`: no top of book ticks.
    pub(crate) top_off: bool,
    /// An index streamed in real time: the reference gives it from its
    /// native top, with no bid, ask or sizes (ibx#640).
    index_top: bool,
}

/// What a new request is.
pub(crate) struct NewRequest<'a> {
    /// A plain snapshot, of this security type.
    pub snapshot: Option<&'a str>,
    pub top_off: bool,
    pub farm_auto: bool,
    /// An index streamed in real time (ibx#640).
    pub index_top: bool,
}

/// An instrument as the queued steps have it: the reference's record at
/// the step being read.
#[derive(Debug, Clone, Copy)]
struct Mirror {
    fields: [i64; field::COUNT],
    halted: Option<i64>,
    auto: QuoteMarks,
    /// Sizes given once (`SizeKind` bits).
    seen: u8,
    /// False while the catch-up of a quote taken over with data is awaited.
    valid: bool,
    /// A new subscription of the instrument from this queue position: the
    /// record starts empty there (valid or not), after the steps of the
    /// earlier one.
    reset_at: Option<(u64, bool)>,
}

impl Default for Mirror {
    fn default() -> Self {
        Self { fields: [0; field::COUNT], halted: None, auto: QuoteMarks::default(), seen: 0, valid: true, reset_at: None }
    }
}

/// The size kind of a size field.
fn size_kind(idx: usize) -> Option<SizeKind> {
    match idx {
        field::BID_SIZE => Some(SizeKind::Bid),
        field::ASK_SIZE => Some(SizeKind::Ask),
        field::LAST_SIZE => Some(SizeKind::Last),
        field::VOLUME => Some(SizeKind::Volume),
        _ => None,
    }
}

/// The price of the side of a kept step's size field.
fn kept_price(ev: &MdEvent, size_idx: usize) -> i64 {
    match size_idx {
        field::BID_SIZE => ev.values[field::BID],
        field::ASK_SIZE => ev.values[field::ASK],
        _ => 0,
    }
}

impl Mirror {
    fn apply(&mut self, ev: &MdEvent) {
        let mut set = ev.fields;
        while set != 0 {
            let i = set.trailing_zeros() as usize;
            set &= set - 1;
            self.fields[i] = ev.values[i];
            // A catch-up says which sizes came. A kept -1 has no size: the
            // farm's first size, 0 too, is then given (ibx#647).
            if ev.step != MdStep::CatchUp && let Some(kind) = size_kind(i)
                && !(ev.step == MdStep::KeptQuote && kept_price(ev, i) < 0)
            {
                self.seen |= 1 << kind as u8;
            }
        }
        if ev.step == MdStep::CatchUp {
            self.seen = ev.seen;
        }
        if let Some(h) = ev.halted {
            self.halted = Some(h as i64);
        }
        let given = QuoteMarks::from_auto_word(ev.auto);
        if let Some(on) = given.bid_auto() {
            self.auto.set_bid_auto(on);
        }
        if let Some(on) = given.ask_auto() {
            self.auto.set_ask_auto(on);
        }
        if ev.step == MdStep::CatchUp {
            self.valid = true;
        }
    }

    fn seen(&self, kind: SizeKind) -> bool {
        self.seen & (1 << kind as u8) != 0
    }

    /// The new subscription's empty record, once the steps before its
    /// place are read. True when done.
    fn reset_if_due(&mut self, seq: u64) -> bool {
        match self.reset_at {
            Some((at, valid)) if seq >= at => {
                *self = Mirror { valid, ..Mirror::default() };
                true
            }
            _ => false,
        }
    }
}

/// A cancelled request: the steps before its cancel are still its own.
struct Retired {
    req_id: i64,
    instrument: InstrumentId,
    /// The queue position at the cancel.
    end: u64,
    st: StreamState,
    snap: Option<PlainSnapshot>,
    /// Its generic ticks (ibx#450).
    generic: Vec<i32>,
}

/// The reader's side of the queue: the instruments' records, and the
/// requests cancelled with steps still to read.
pub(crate) struct MdReader {
    mirrors: Vec<Mirror>,
    /// Records with a reset to make.
    resets: usize,
    retired: Vec<Retired>,
}

impl MdReader {
    pub(crate) fn new() -> Self {
        Self { mirrors: vec![Mirror::default(); MAX_INSTRUMENTS], resets: 0, retired: Vec::new() }
    }
}

/// The fields of a stream step, in the order the reference sends them
/// (the last trade's exchange between the quote's exchanges and the time:
/// reference 10.51, paper 12/10/2026, regular hours; ibx#638).
const STEP_ALL: [usize; 15] = [
    field::BID, field::ASK, field::LAST, field::BID_SIZE, field::ASK_SIZE, field::LAST_SIZE, field::VOLUME,
    field::HIGH, field::LOW, field::CLOSE, field::OPEN, field::BID_EXCH, field::ASK_EXCH, field::LAST_EXCH,
    field::TIME,
];
/// The delayed data sender's order: the volume after the low; it sends no
/// exchanges (skipped for delayed data).
const STEP_ALL_DELAYED: [usize; 15] = [
    field::BID, field::ASK, field::LAST, field::BID_SIZE, field::ASK_SIZE, field::LAST_SIZE, field::HIGH,
    field::LOW, field::VOLUME, field::CLOSE, field::OPEN, field::BID_EXCH, field::ASK_EXCH, field::LAST_EXCH,
    field::TIME,
];

/// Which step a request is given.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Its first step after it joined a quote: all of it.
    Join,
    /// A queued step.
    Step(MdStep, u16),
}

/// The callbacks of one step of one request being written.
struct Out<'a> {
    out: &'a mut Vec<MdOut>,
    req_id: i64,
    fields: &'a [i64; field::COUNT],
    delayed: bool,
    shared: &'a SharedState,
    instrument: InstrumentId,
    /// The queue position of the step.
    seq: u64,
}

impl Out<'_> {
    fn api(&self, tick_type: i32) -> i32 {
        if self.delayed { delayed_tick_type(tick_type) } else { tick_type }
    }

    fn push(&mut self, tick: MdTick) {
        self.out.push(MdOut::Tick(self.req_id, tick));
    }

    fn price(&mut self, tick_type: i32, idx: usize, can_auto_execute: bool) {
        let tick_type = self.api(tick_type);
        self.push(MdTick::Price { tick_type, value: self.fields[idx] as f64 / PRICE_SCALE_F, can_auto_execute });
    }

    /// A price or size the record does not have: unset (`f64::MAX`).
    fn unset_price(&mut self, tick_type: i32) {
        let tick_type = self.api(tick_type);
        self.push(MdTick::Price { tick_type, value: f64::MAX, can_auto_execute: false });
    }

    fn unset_size(&mut self, tick_type: i32) {
        let tick_type = self.api(tick_type);
        self.push(MdTick::Size { tick_type, value: f64::MAX });
    }

    fn size(&mut self, tick_type: i32, idx: usize) {
        let tick_type = self.api(tick_type);
        self.push(MdTick::Size { tick_type, value: self.fields[idx] as f64 / QTY_SCALE as f64 });
    }

    fn time(&mut self) {
        let tick_type = self.api(TICK_LAST_TIMESTAMP);
        self.push(MdTick::Time { tick_type, secs: self.fields[field::TIME] / 1_000_000_000 });
    }

    /// An exchange text; an empty one is not sent: no letters before the
    /// exchange map came.
    fn exchanges(&mut self, idx: usize) {
        let tick_type = match idx {
            field::BID_EXCH => TICK_BID_EXCHANGE,
            field::ASK_EXCH => TICK_ASK_EXCHANGE,
            _ => TICK_LAST_EXCHANGE,
        };
        let value = render_exchange_mask_at(self.fields[idx], self.instrument, self.shared, self.seq);
        if !value.is_empty() {
            self.push(MdTick::Text { tick_type, value });
        }
    }

    fn halted(&mut self, status: i64) {
        let tick_type = self.api(TICK_HALTED);
        self.push(MdTick::Generic { tick_type, value: QuoteMarks::halted_tick_value(status) });
    }
}

/// A last price the reference gives: above 0, or with a size (ibx#671).
fn last_valid(fields: &[i64; field::COUNT]) -> bool {
    fields[field::LAST] > 0 || fields[field::LAST_SIZE] != 0
}

/// A stream's step: the fields that changed, in the sender's order, then
/// the halted state while it is pending.
fn stream_pass(st: &mut StreamState, m: &Mirror, pass: Pass, out: &mut Out) {
    let fields = &m.fields;
    let mut todo = [false; field::COUNT];
    for (idx, pending) in todo.iter_mut().enumerate() {
        *pending = match idx {
            field::BID_SIZE | field::ASK_SIZE | field::LAST_SIZE | field::VOLUME => {
                let kind = size_kind(idx).unwrap_or(SizeKind::Volume);
                (m.seen(kind) || fields[idx] != 0)
                    && (st.sizes_sent & (1 << kind as u8) == 0 || fields[idx] != st.fields[idx])
            }
            // Each exchange when it changes: the last trade's exchange is
            // not given again for a trade on the same exchange (ibx#638).
            field::BID_EXCH | field::ASK_EXCH | field::LAST_EXCH => fields[idx] != st.fields[idx],
            _ => fields[idx] != 0 && fields[idx] != st.fields[idx],
        };
    }
    if let Some(status) = m.halted {
        let bits = (status & 3) as u8;
        if bits != st.halted_bits {
            st.halted_bits = bits;
            st.halted_pending = true;
        }
    }
    let delayed = st.delayed;
    let (bid_auto, ask_auto) = if delayed {
        (false, false)
    } else if st.farm_auto {
        (m.auto.bid_auto() == Some(true), m.auto.ask_auto() == Some(true))
    } else {
        (true, true)
    };
    // A kept bid or ask of -1 (no quote when the contract was left) comes
    // with an unset size and no auto-execution (ibx#647; the reference
    // 10.51, paper, 13/10/2026 pre-market, two option streams).
    let kept = matches!(pass, Pass::Step(MdStep::KeptQuote, _));
    for &idx in if delayed { &STEP_ALL_DELAYED } else { &STEP_ALL } {
        if !todo[idx] {
            continue;
        }
        if kept && matches!(idx, field::BID | field::ASK) && fields[idx] < 0 {
            let (pt, size_idx, size_tt) = if idx == field::BID {
                (TICK_BID, field::BID_SIZE, TICK_BID_SIZE)
            } else {
                (TICK_ASK, field::ASK_SIZE, TICK_ASK_SIZE)
            };
            out.price(pt, idx, false);
            out.unset_size(size_tt);
            todo[size_idx] = false;
            continue;
        }
        // An index from its native top (ibx#640; the reference 10.51, SPX
        // on CBOE, 13/10/2026, regular hours): bid and ask unset with
        // their sizes, the last with an unset size, no size or volume of
        // its own; the rest as for any contract.
        if st.index_top {
            match idx {
                field::BID => { out.unset_price(TICK_BID); out.unset_size(TICK_BID_SIZE); continue; }
                field::ASK => { out.unset_price(TICK_ASK); out.unset_size(TICK_ASK_SIZE); continue; }
                field::LAST => { out.price(TICK_LAST, idx, false); out.unset_size(TICK_LAST_SIZE); continue; }
                field::BID_SIZE | field::ASK_SIZE | field::LAST_SIZE | field::VOLUME => continue,
                _ => {}
            }
        }
        match idx {
            // A price comes with its size; the delayed sender does not send
            // that size again on its own (captured 05/10/2026, 7203 on
            // delayed data: 66 and 69, 67 and 70, 68 and 71, once each).
            field::BID => { out.price(TICK_BID, idx, bid_auto); out.size(TICK_BID_SIZE, field::BID_SIZE); if delayed { todo[field::BID_SIZE] = false; } }
            field::ASK => { out.price(TICK_ASK, idx, ask_auto); out.size(TICK_ASK_SIZE, field::ASK_SIZE); if delayed { todo[field::ASK_SIZE] = false; } }
            // No trade: a last of -1 with no size is not given; the size
            // goes on its own (ibx#671; the reference 10.51, an option with
            // no trade, 13/10/2026).
            field::LAST if !last_valid(fields) => {}
            field::LAST => { out.price(TICK_LAST, idx, false); out.size(TICK_LAST_SIZE, field::LAST_SIZE); if delayed { todo[field::LAST_SIZE] = false; } }
            field::BID_SIZE => out.size(TICK_BID_SIZE, idx),
            field::ASK_SIZE => out.size(TICK_ASK_SIZE, idx),
            field::LAST_SIZE => out.size(TICK_LAST_SIZE, idx),
            field::VOLUME => out.size(TICK_VOLUME, idx),
            field::HIGH => out.price(TICK_HIGH, idx, false),
            field::LOW => out.price(TICK_LOW, idx, false),
            field::CLOSE => out.price(TICK_CLOSE, idx, false),
            field::OPEN => out.price(TICK_OPEN, idx, false),
            field::TIME => out.time(),
            // The delayed sender sends no exchanges.
            field::BID_EXCH | field::ASK_EXCH | field::LAST_EXCH if delayed => {}
            field::BID_EXCH | field::ASK_EXCH | field::LAST_EXCH => out.exchanges(idx),
            _ => {}
        }
    }
    // A request that joins gets the halted state when one is known (the
    // reference marks it when the request joins the record).
    if pass == Pass::Join {
        st.halted_pending = m.halted.is_some();
    }
    if st.halted_pending && let Some(status) = m.halted {
        out.halted(status);
    }
    // A book update clears every mark.
    if matches!(pass, Pass::Step(MdStep::Quote | MdStep::KeptQuote | MdStep::CatchUp, _)) {
        st.halted_pending = false;
    }
    st.fields = *fields;
    for idx in [field::BID_SIZE, field::ASK_SIZE, field::LAST_SIZE, field::VOLUME] {
        if let Some(kind) = size_kind(idx)
            && (m.seen(kind) || fields[idx] != 0)
        {
            st.sizes_sent |= 1 << kind as u8;
        }
    }
}

/// A plain snapshot's step: each tick type once, from the fields the step
/// set (all of them when the request joins or catches up), in the sender's
/// order; the halted state from a trade step, from the first status known,
/// as the reference's snapshot record marks it. A snapshot's bid or ask
/// executes automatically only when the farm said so. No bid or ask
/// exchange text: the reference sent none in 12 stock snapshots with the
/// letters on the quote (10.51, paper, 12/10/2026, regular hours; ibx#503).
/// The delayed sender sends no halted state either.
fn snapshot_pass(snap: &mut PlainSnapshot, m: &Mirror, pass: Pass, out: &mut Out) {
    let (set, trade) = match pass {
        Pass::Join | Pass::Step(MdStep::CatchUp, _) => (field::ALL, true),
        Pass::Step(step, set) => (set, step == MdStep::Last),
    };
    let delayed = out.delayed;
    let f = m.fields;
    let has = |idx: usize| set & (1 << idx) != 0 && f[idx] != 0;
    let kept = matches!(pass, Pass::Step(MdStep::KeptQuote, _));
    // An index (ibx#680; the reference 10.51, SPX on CBOE, 13/10/2026,
    // regular hours): a time of 0, high, low and close unset, then the bid
    // and ask with their sizes; never a last, its size, the volume, the
    // open or the halted state, so it ends at the time limit.
    if snap.index && !delayed {
        if snap.take(TICK_LAST_TIMESTAMP) {
            out.push(MdTick::Time { tick_type: TICK_LAST_TIMESTAMP, secs: 0 });
        }
        for tt in [TICK_HIGH, TICK_LOW, TICK_CLOSE] {
            if snap.take(tt) {
                out.unset_price(tt);
            }
        }
        for (idx, size, pt, st) in [(field::BID, field::BID_SIZE, TICK_BID, TICK_BID_SIZE), (field::ASK, field::ASK_SIZE, TICK_ASK, TICK_ASK_SIZE)] {
            if has(idx) && snap.take(pt) {
                out.price(pt, idx, false);
                out.size(st, size);
            }
        }
        return;
    }
    for (idx, size, pt, st, auto) in [
        (field::BID, field::BID_SIZE, TICK_BID, TICK_BID_SIZE, m.auto.bid_auto()),
        (field::ASK, field::ASK_SIZE, TICK_ASK, TICK_ASK_SIZE, m.auto.ask_auto()),
    ] {
        if has(idx) && snap.take(out.api(pt)) {
            // Kept values carry no auto-execution for a snapshot (ibx#671;
            // the reference 10.51: false in a snapshot, true in a stream).
            out.price(pt, idx, !delayed && !kept && auto == Some(true));
            // A kept -1 has no size (ibx#647; the reference 10.51, paper,
            // 12/10/2026 after hours, an option snapshot).
            if kept && f[idx] < 0 {
                out.unset_size(st);
            } else {
                out.size(st, size);
            }
        }
    }
    if has(field::LAST) && last_valid(&f) && snap.take(out.api(TICK_LAST)) {
        out.price(TICK_LAST, field::LAST, false);
        out.size(TICK_LAST_SIZE, field::LAST_SIZE);
    }
    // An option's volume of 0 is given too (ibx#681; the reference 10.51,
    // an option with no trade, 13/10/2026).
    let option = snap.is_option();
    let volume = (has(field::VOLUME) || (option && set & (1 << field::VOLUME) != 0 && m.seen(SizeKind::Volume)))
        && snap.take(out.api(TICK_VOLUME));
    if volume && !delayed {
        out.size(TICK_VOLUME, field::VOLUME);
    }
    for (idx, tt) in [(field::HIGH, TICK_HIGH), (field::LOW, TICK_LOW)] {
        if has(idx) && snap.take(out.api(tt)) {
            out.price(tt, idx, false);
        }
    }
    if volume && delayed {
        out.size(TICK_VOLUME, field::VOLUME);
    }
    for (idx, tt) in [(field::CLOSE, TICK_CLOSE), (field::OPEN, TICK_OPEN)] {
        if has(idx) && snap.take(out.api(tt)) {
            out.price(tt, idx, false);
        }
    }
    // An option's bid and ask exchanges (ibx#681; the reference 10.51,
    // 13/10/2026: in option snapshots, not in stock ones, ibx#503).
    if option && !delayed {
        for (idx, tt) in [(field::BID_EXCH, TICK_BID_EXCHANGE), (field::ASK_EXCH, TICK_ASK_EXCHANGE)] {
            if has(idx) && snap.take(tt) {
                out.exchanges(idx);
            }
        }
    }
    if has(field::TIME) && snap.take(out.api(TICK_LAST_TIMESTAMP)) {
        out.time();
    }
    // An option snapshot gives no halted state (ibx#647; the reference
    // 10.51, 12/10/2026 after hours and 13/10/2026 regular hours).
    if trade && !delayed && !option && let Some(status) = m.halted && snap.take(TICK_HALTED) {
        out.halted(status);
    }
}

impl ClientCore {
    /// Set up the stream state of a new request and, for the first request
    /// of an instrument, the instrument's record and its steps (ibx#446).
    /// The request reads the queue from its current end, or from `at`. A
    /// request that joins (another request's quote, or the P&L quote,
    /// `had_data`) gets all of the quote first; the P&L quote's comes from
    /// a catch-up the engine writes, as no step of it was queued. Called
    /// with the instrument's requests locked.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_md_stream(
        &self, shared: &SharedState, req_id: i64, instrument: InstrumentId, joining: bool, had_data: bool,
        kind: Option<NewRequest>, at: Option<u64>,
    ) {
        let queue = &shared.market.md_events;
        let head = at.unwrap_or_else(|| queue.head());
        if !joining {
            // A new subscription starts with no generic value known (ibx#495).
            self.md_generic_last.lock().unwrap().remove(&instrument);
            let mut reader = self.md_reader.lock().unwrap();
            let MdReader { mirrors, resets, .. } = &mut *reader;
            if let Some(m) = mirrors.get_mut(instrument as usize) {
                if m.reset_at.is_none() {
                    *resets += 1;
                }
                m.reset_at = Some((head, !had_data));
            }
            drop(reader);
            queue.listen(instrument, true);
            if had_data {
                queue.request_catch_up(instrument);
            }
        }
        if let Some(sec_type) = kind.as_ref().and_then(|k| k.snapshot) {
            let mut snaps = self.snapshot_reqs.lock().unwrap();
            snaps.insert(req_id, PlainSnapshot::new(sec_type, std::time::Instant::now()));
            self.snapshot_count.store(snaps.len(), Ordering::Release);
        }
        let delayed = self.delayed_reqs.lock().unwrap().contains(&req_id);
        let mut streams = self.last_quotes.lock().unwrap();
        let old = streams.remove(&req_id).unwrap_or_default();
        if old.waiting_join {
            self.md_joins_waiting.fetch_sub(1, Ordering::AcqRel);
        }
        let waiting_join = joining || had_data;
        if waiting_join {
            self.md_joins_waiting.fetch_add(1, Ordering::AcqRel);
        }
        let (top_off, farm_auto, index_top) = kind.map_or((old.top_off, old.farm_auto, old.index_top),
            |k| (k.top_off, k.farm_auto, k.index_top));
        streams.insert(req_id, StreamState {
            start: head, waiting_join, delayed, top_off, farm_auto, index_top, mdt_done: old.mdt_done, ..StreamState::default()
        });
    }

    /// A request joins a quote that already has data, from the current end
    /// of the queue (tests).
    #[cfg(test)]
    pub(crate) fn join_stream(&self, req_id: i64, head: u64) {
        let mut streams = self.last_quotes.lock().unwrap();
        let st = streams.entry(req_id).or_default();
        if !st.waiting_join {
            self.md_joins_waiting.fetch_add(1, Ordering::AcqRel);
        }
        st.waiting_join = true;
        st.start = head;
    }

    /// A request ends. Cancelled by the client (`cancelled`), the steps
    /// queued until now are still its own (its snapshot too). Called with
    /// the instrument's requests locked.
    pub(super) fn end_md_stream(&self, shared: &SharedState, req_id: i64, instrument: InstrumentId, cancelled: bool) {
        let st = self.last_quotes.lock().unwrap().remove(&req_id);
        let snap = if self.snapshot_count.load(Ordering::Acquire) > 0 {
            let mut snaps = self.snapshot_reqs.lock().unwrap();
            let snap = snaps.remove(&req_id);
            self.snapshot_count.store(snaps.len(), Ordering::Release);
            snap
        } else {
            None
        };
        let Some(st) = st else { return };
        if st.waiting_join {
            self.md_joins_waiting.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let end = shared.market.md_events.head();
        let generic = self.md_generic.lock().unwrap().get(&req_id).map(|g| g.codes.clone()).unwrap_or_default();
        if cancelled && !st.ended && (end > shared.market.md_events.tail() || !generic.is_empty()) {
            self.md_reader.lock().unwrap().retired.push(Retired { req_id, instrument, end, st, snap, generic });
        }
    }

    /// The market data callbacks of the steps queued since the last call,
    /// in their order, for every request (see the module), and the ends of
    /// the plain snapshots that completed or reached their time limit
    /// (11 s, at `now`, the current time when None). A request with
    /// `mdoff` gets no top of book tick; its snapshot still ends as the
    /// quote completes (ibx#444).
    pub fn poll_market_data(&self, shared: &SharedState, now: Option<std::time::Instant>, out: &mut Vec<MdOut>) {
        // A snapshot asked with no type takes its definition's (ibx#676).
        for (instrument, kind) in shared.market.drain_md_kinds() {
            let reqs: Vec<i64> = self.instrument_to_req.lock().unwrap().get(&instrument).cloned().unwrap_or_default();
            let mut snaps = self.snapshot_reqs.lock().unwrap();
            for req in reqs {
                if let Some(snap) = snaps.get_mut(&req) {
                    snap.retype(&kind);
                }
            }
        }
        let queue = &shared.market.md_events;
        let head = queue.head();
        let tail = queue.tail();
        let snapshots = self.snapshot_count.load(Ordering::Acquire) > 0;
        // The generic tick blocks read before the queue's end (ibx#450).
        let generic = shared.market.take_generic_ticks(head);
        if head == tail && !snapshots && self.md_joins_waiting.load(Ordering::Acquire) == 0 && generic.is_empty() {
            return;
        }
        let mut generic = generic.into_iter().peekable();
        let observers = self.instrument_to_req.lock().unwrap();
        let mut reader = self.md_reader.lock().unwrap();
        let mut streams = self.last_quotes.lock().unwrap();
        let mut snaps = self.snapshot_reqs.lock().unwrap();
        let MdReader { mirrors, resets, retired } = &mut *reader;
        let mut d = Drain { core: self, shared, out, streams: &mut streams, snaps: &mut snaps };
        let mut step = |seq: u64, g: Option<&crate::bridge::GenericTicks>| {
            if let Some(g) = g {
                d.generic(g, observers.get(&g.instrument).map_or(&[][..], Vec::as_slice), retired);
                return;
            }
            let ev = queue.read(seq);
            let instrument = ev.instrument;
            let Some(m) = mirrors.get_mut(instrument as usize) else { return };
            if m.reset_if_due(seq) {
                *resets -= 1;
            }
            // A new subscription's steps are its requests'; the steps of
            // an earlier one are only for the requests cancelled after them.
            let reqs = match m.reset_at {
                Some(_) => &[][..],
                None => observers.get(&instrument).map_or(&[][..], Vec::as_slice),
            };
            // Requests that joined before this step: all of the quote first.
            if m.valid && self.md_joins_waiting.load(Ordering::Relaxed) > 0 {
                for &req_id in reqs {
                    d.join_if_due(req_id, instrument, m, seq, reqs);
                }
            }
            m.apply(&ev);
            // The newest request first, as the reference notifies the
            // subscribers of a record (captured 05/10/2026, two AAPL
            // requests: each step went to the second one, then the first).
            for &req_id in reqs.iter().rev() {
                d.step(req_id, instrument, m, &ev, seq);
            }
            for r in retired.iter_mut().filter(|r| r.instrument == instrument && seq < r.end && seq >= r.st.start) {
                if !r.st.waiting_join {
                    d.retired_step(r, m, &ev, seq);
                }
            }
        };
        // Message by message: its book updates after its other steps, as
        // the reference applies them once it read the message; the generic
        // tick blocks between the messages they came between (ibx#450).
        let mut start = tail;
        while start < head {
            while let Some(g) = generic.next_if(|g| g.at <= start) {
                step(start, Some(&g));
            }
            let mut end = start + 1;
            let mut books = false;
            let mut others = false;
            for seq in start..head {
                let (kind, first) = queue.step_at(seq);
                if seq > start && first {
                    break;
                }
                end = seq + 1;
                if matches!(kind, MdStep::Quote | MdStep::KeptQuote) { books = true } else { others = true }
            }
            if books && others {
                for quotes in [false, true] {
                    for seq in start..end {
                        if matches!(queue.step_at(seq).0, MdStep::Quote | MdStep::KeptQuote) == quotes {
                            step(seq, None);
                        }
                    }
                }
            } else {
                for seq in start..end {
                    step(seq, None);
                }
            }
            start = end;
        }
        for g in generic {
            step(head, Some(&g));
        }
        queue.release(head);
        retired.clear();
        // New subscriptions with no step yet, and the requests that joined
        // after the last step.
        if *resets > 0 {
            for m in mirrors.iter_mut() {
                if m.reset_if_due(head) {
                    *resets -= 1;
                }
            }
        }
        if self.md_joins_waiting.load(Ordering::Acquire) > 0 {
            for (&instrument, reqs) in observers.iter() {
                let Some(m) = mirrors.get(instrument as usize).filter(|m| m.valid && m.reset_at.is_none()) else { continue };
                for &req_id in reqs {
                    d.join_if_due(req_id, instrument, m, head, reqs);
                }
            }
        }
        if snapshots {
            d.snapshot_timeouts(now.unwrap_or_else(std::time::Instant::now), &observers);
        }
    }
}

/// The API tick type of an option computation: delayed data has its own.
fn option_tick_type(tick_type: i32, delayed: bool) -> i32 {
    if delayed { market::delayed(tick_type) } else { tick_type }
}

/// The callback of an option computation.
fn option_tick(c: &OptionComputation, delayed: bool) -> MdTick {
    MdTick::Option(OptionComputation { tick_type: option_tick_type(c.tick_type, delayed), ..*c })
}

/// The option computations of an instrument's model as last given, the
/// model's first (ibx#502).
fn known_computations(core: &ClientCore, instrument: InstrumentId) -> Vec<OptionComputation> {
    use crate::control::generic_values::GenTick;
    let last = core.md_generic_last.lock().unwrap();
    let Some(kept) = last.get(&instrument) else { return Vec::new() };
    let mut out: Vec<OptionComputation> = kept.values()
        .filter_map(|(code, t)| match t {
            GenTick::Option(c) if *code == market::COMPUTATIONS => Some(*c),
            _ => None,
        })
        .collect();
    out.sort_by_key(|c| if c.tick_type == market::MODEL { 0 } else { c.tick_type });
    out
}

/// One step of one request: its callbacks, the market data type before its
/// first tick, and a snapshot's end once complete (true then).
#[allow(clippy::too_many_arguments)]
fn pass_one(
    core: &ClientCore, shared: &SharedState, out: &mut Vec<MdOut>, req_id: i64, st: &mut StreamState,
    snap: Option<&mut PlainSnapshot>, instrument: InstrumentId, m: &Mirror, pass: Pass, seq: u64,
) -> bool {
    if st.ended {
        return false;
    }
    // What the contract kept is the request's baseline: not sent (ibx#508).
    if let Pass::Step(MdStep::Kept, set) = pass {
        for idx in (0..field::COUNT).filter(|i| set & (1 << i) != 0) {
            st.fields[idx] = m.fields[idx];
        }
        return false;
    }
    let mark = out.len();
    let complete = {
        let mut o = Out { out: &mut *out, req_id, fields: &m.fields, delayed: st.delayed, shared, instrument, seq };
        match snap {
            Some(snap) => {
                snapshot_pass(snap, m, pass, &mut o);
                snap.complete()
            }
            None => {
                stream_pass(st, m, pass, &mut o);
                false
            }
        }
    };
    // `mdoff`: the reference's sender skips the top of book pass of the
    // request.
    if st.top_off {
        out.truncate(mark);
    }
    if out.len() > mark && !st.mdt_done {
        st.mdt_done = true;
        if let Some(mdt) = core.check_mdt_needed(req_id, true) {
            out.insert(mark, MdOut::MarketDataType(req_id, mdt));
        }
    }
    if complete {
        st.ended = true;
        out.push(MdOut::SnapshotEnd(req_id));
    }
    complete
}

/// The dispatch reading the queue, with the request states locked.
struct Drain<'a> {
    core: &'a ClientCore,
    shared: &'a SharedState,
    out: &'a mut Vec<MdOut>,
    streams: &'a mut HashMap<i64, StreamState>,
    snaps: &'a mut HashMap<i64, PlainSnapshot>,
}

impl Drain<'_> {
    /// The API ticks of a generic tick block (ibx#450) to the requests of
    /// the instrument that asked that tick (RTVolume and RT trade volume:
    /// either), made before the block came; a cancelled request gets the
    /// blocks that came before its cancel.
    fn generic(&mut self, g: &crate::bridge::GenericTicks, reqs: &[i64], retired: &[Retired]) {
        use crate::control::generic_values::{GenTick, RT_TRADE_VOLUME, RT_VOLUME};
        if g.code == market::COMPUTATIONS {
            return self.computations(g, reqs, retired);
        }
        if g.code == market::COMPUTATIONS_KEPT {
            // Kept for the requests that join and the snapshots at their
            // limit only (ibx#502).
            let mut last = self.core.md_generic_last.lock().unwrap();
            let kept = last.entry(g.instrument).or_default();
            for t in &g.ticks {
                if let GenTick::Option(c) = t {
                    kept.insert(c.tick_type, (market::COMPUTATIONS, t.clone()));
                }
            }
            return;
        }
        let asked = |codes: &[i32]| {
            codes.contains(&g.code)
                || (matches!(g.code, RT_VOLUME | RT_TRADE_VOLUME) && codes.iter().any(|c| matches!(*c, RT_VOLUME | RT_TRADE_VOLUME)))
        };
        // Kept for the requests that join (ibx#495).
        {
            let mut last = self.core.md_generic_last.lock().unwrap();
            let kept = last.entry(g.instrument).or_default();
            for t in &g.ticks {
                let tick_type = match t {
                    GenTick::Price(t, _) | GenTick::Size(t, _) | GenTick::Generic(t, _) | GenTick::Text(t, _) => *t,
                    GenTick::Option(c) => c.tick_type,
                };
                kept.insert(tick_type, (g.code, t.clone()));
            }
        }
        let wanted: Vec<i64> = {
            let generic = self.core.md_generic.lock().unwrap();
            let live = reqs.iter().rev().copied()
                .filter(|r| self.streams.get(r).is_none_or(|st| st.start <= g.at && !st.ended))
                .filter(|r| generic.get(r).is_some_and(|g| asked(&g.codes)));
            let gone = retired.iter()
                .filter(|r| r.instrument == g.instrument && g.at < r.end && r.st.start <= g.at && asked(&r.generic))
                .map(|r| r.req_id);
            live.chain(gone).collect()
        };
        for req_id in wanted {
            for t in &g.ticks {
                let tick = match t.clone() {
                    GenTick::Price(tick_type, value) => MdTick::Price { tick_type, value, can_auto_execute: false },
                    GenTick::Size(tick_type, value) => MdTick::Size { tick_type, value },
                    GenTick::Generic(tick_type, value) => MdTick::Generic { tick_type, value },
                    GenTick::Text(tick_type, value) => MdTick::Text { tick_type, value },
                    GenTick::Option(c) => MdTick::Option(c),
                };
                self.out.push(MdOut::Tick(req_id, tick));
            }
        }
    }

    /// The join step of a request whose place in the queue is reached:
    /// the requests of the instrument already streaming get all of the
    /// quote again, then the one that joins, each with the generic values
    /// it asked that are known, as the reference gives a record's picture
    /// to every subscriber when one joins (the reference 10.51, paper
    /// 12/10/2026: two joins, the running request first in one, last in
    /// the other; ibx#495).
    fn join_if_due(&mut self, req_id: i64, instrument: InstrumentId, m: &Mirror, seq: u64, reqs: &[i64]) {
        let st = self.streams.entry(req_id).or_default();
        if !(st.waiting_join && seq >= st.start) {
            return;
        }
        st.waiting_join = false;
        self.core.md_joins_waiting.fetch_sub(1, Ordering::AcqRel);
        let running: Vec<i64> = reqs.iter().copied().filter(|&r| r != req_id && !self.snaps.contains_key(&r))
            .filter(|r| self.streams.get(r).is_some_and(|s| !s.waiting_join && !s.ended && s.mdt_done && s.start <= seq))
            .collect();
        for r in running.into_iter().chain(std::iter::once(req_id)) {
            let Some(st) = self.streams.get_mut(&r) else { continue };
            if r != req_id {
                // Sent again from nothing, as the request that joins.
                st.fields = [0; field::COUNT];
                st.sizes_sent = 0;
                st.halted_bits = 0;
                st.halted_pending = false;
            }
            let mark = self.out.len();
            Self::pass(self.core, self.shared, self.out, self.snaps, r, st, instrument, m, Pass::Join, seq);
            if self.out.len() > mark {
                self.known_generic(r, instrument);
            }
        }
    }

    /// The known generic values of `instrument` that request `req_id`
    /// asked, by tick type (ibx#495), after its option computations
    /// (ibx#502).
    fn known_generic(&mut self, req_id: i64, instrument: InstrumentId) {
        use crate::control::generic_values::{GenTick, RT_TRADE_VOLUME, RT_VOLUME};
        let comps = known_computations(self.core, instrument);
        if !comps.is_empty() {
            self.emit_computations(req_id, &comps);
        }
        let codes = self.core.md_generic.lock().unwrap().get(&req_id).map(|g| g.codes.clone()).unwrap_or_default();
        if codes.is_empty() {
            return;
        }
        let last = self.core.md_generic_last.lock().unwrap();
        let Some(kept) = last.get(&instrument) else { return };
        for (code, t) in kept.values() {
            // The real-time volume is a trade, not a value to restate.
            if matches!(*code, RT_VOLUME | RT_TRADE_VOLUME) || !codes.contains(code) {
                continue;
            }
            let tick = match t.clone() {
                GenTick::Price(tick_type, value) => MdTick::Price { tick_type, value, can_auto_execute: false },
                GenTick::Size(tick_type, value) => MdTick::Size { tick_type, value },
                GenTick::Generic(tick_type, value) => MdTick::Generic { tick_type, value },
                GenTick::Text(tick_type, value) => MdTick::Text { tick_type, value },
                GenTick::Option(_) => continue,
            };
            self.out.push(MdOut::Tick(req_id, tick));
        }
    }

    /// The option computations of an option's model (ibx#502) to the
    /// requests of the instrument made before they came (a cancelled one:
    /// those before its cancel). Kept for the requests that join and for
    /// the snapshots that reach their time limit.
    fn computations(&mut self, g: &crate::bridge::GenericTicks, reqs: &[i64], retired: &[Retired]) {
        use crate::control::generic_values::GenTick;
        let comps: Vec<OptionComputation> = g.ticks.iter()
            .filter_map(|t| if let GenTick::Option(c) = t { Some(*c) } else { None })
            .collect();
        {
            let mut last = self.core.md_generic_last.lock().unwrap();
            let kept = last.entry(g.instrument).or_default();
            for c in &comps {
                kept.insert(c.tick_type, (g.code, GenTick::Option(*c)));
            }
        }
        let live: Vec<i64> = reqs.iter().rev().copied()
            .filter(|r| self.streams.get(r).is_none_or(|st| st.start <= g.at && !st.ended && !st.waiting_join))
            .collect();
        for req_id in live {
            self.emit_computations(req_id, &comps);
        }
        for r in retired.iter().filter(|r| r.instrument == g.instrument && g.at < r.end && r.st.start <= g.at && !r.st.ended) {
            for c in &comps {
                self.out.push(MdOut::Tick(r.req_id, option_tick(c, r.st.delayed)));
            }
        }
    }

    /// Option computations for one request: a stream gets each; a plain
    /// snapshot takes each tick type once, the model's at once and the
    /// others once they have a volatility or a greek (`optionmodel.al`),
    /// and ends when complete.
    fn emit_computations(&mut self, req_id: i64, comps: &[OptionComputation]) {
        let st = self.streams.entry(req_id).or_default();
        if st.ended {
            return;
        }
        let mark = self.out.len();
        let delayed = st.delayed;
        let mut complete = false;
        match self.snaps.get_mut(&req_id) {
            Some(snap) => {
                for c in comps {
                    if (c.tick_type == market::MODEL || market::valid(c)) && snap.take(option_tick_type(c.tick_type, delayed)) {
                        self.out.push(MdOut::Tick(req_id, option_tick(c, delayed)));
                    }
                }
                complete = snap.complete();
            }
            None => {
                for c in comps {
                    self.out.push(MdOut::Tick(req_id, option_tick(c, delayed)));
                }
            }
        }
        if self.out.len() > mark && !st.mdt_done {
            st.mdt_done = true;
            if let Some(mdt) = self.core.check_mdt_needed(req_id, true) {
                self.out.insert(mark, MdOut::MarketDataType(req_id, mdt));
            }
        }
        if complete {
            st.ended = true;
            self.out.push(MdOut::SnapshotEnd(req_id));
            self.snaps.remove(&req_id);
            self.core.snapshot_count.store(self.snaps.len(), Ordering::Release);
        }
    }

    /// A queued step for a request: its callbacks; a request waiting for
    /// the catch-up of a taken-over quote takes it as its join step.
    fn step(&mut self, req_id: i64, instrument: InstrumentId, m: &Mirror, ev: &MdEvent, seq: u64) {
        let st = self.streams.entry(req_id).or_default();
        if seq < st.start || st.ended {
            return;
        }
        let pass = if st.waiting_join {
            if !m.valid {
                return;
            }
            st.waiting_join = false;
            self.core.md_joins_waiting.fetch_sub(1, Ordering::AcqRel);
            Pass::Join
        } else {
            Pass::Step(ev.step, ev.fields)
        };
        Self::pass(self.core, self.shared, self.out, self.snaps, req_id, st, instrument, m, pass, seq);
    }

    /// One step of one request.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        core: &ClientCore, shared: &SharedState, out: &mut Vec<MdOut>, snaps: &mut HashMap<i64, PlainSnapshot>,
        req_id: i64, st: &mut StreamState, instrument: InstrumentId, m: &Mirror, pass: Pass, seq: u64,
    ) {
        let snap = if snaps.is_empty() { None } else { snaps.get_mut(&req_id) };
        if pass_one(core, shared, out, req_id, st, snap, instrument, m, pass, seq) {
            snaps.remove(&req_id);
            core.snapshot_count.store(snaps.len(), Ordering::Release);
        }
    }

    /// A step for a cancelled request, written before its cancel.
    fn retired_step(&mut self, r: &mut Retired, m: &Mirror, ev: &MdEvent, seq: u64) {
        let complete = pass_one(
            self.core, self.shared, self.out, r.req_id, &mut r.st, r.snap.as_mut(), r.instrument, m,
            Pass::Step(ev.step, ev.fields), seq,
        );
        if complete {
            r.snap = None;
        }
    }

    /// The snapshots that reached their time limit end; an option's first
    /// gets the computations it did not take yet, as its model has them
    /// (the reference 10.51, 12/10/2026: a bid computation without
    /// volatility at 11 s, then the end).
    fn snapshot_timeouts(&mut self, now: std::time::Instant, observers: &HashMap<InstrumentId, Vec<i64>>) {
        let streams = &mut *self.streams;
        let out = &mut *self.out;
        let core = self.core;
        self.snaps.retain(|&req_id, snap| {
            if !snap.timed_out(now) {
                return true;
            }
            let delayed = streams.get(&req_id).is_some_and(|st| st.delayed);
            if let Some((&instrument, _)) = observers.iter().find(|(_, reqs)| reqs.contains(&req_id)) {
                for c in known_computations(core, instrument) {
                    if snap.take(option_tick_type(c.tick_type, delayed)) {
                        out.push(MdOut::Tick(req_id, option_tick(&c, delayed)));
                    }
                }
            }
            if let Some(st) = streams.get_mut(&req_id) {
                st.ended = true;
            }
            out.push(MdOut::SnapshotEnd(req_id));
            false
        });
        self.core.snapshot_count.store(self.snaps.len(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::md_events::TestMessage;

    /// The callbacks as text.
    fn lines(out: &[MdOut]) -> Vec<String> {
        out.iter().map(|o| match o {
            MdOut::Tick(r, MdTick::Price { tick_type, value, .. }) => format!("price:{r}:{tick_type}:{value}"),
            MdOut::Tick(r, MdTick::Size { tick_type, value }) => format!("size:{r}:{tick_type}:{value}"),
            MdOut::Tick(r, MdTick::Text { tick_type, value }) => format!("string:{r}:{tick_type}:{value}"),
            MdOut::Tick(r, MdTick::Time { tick_type, secs }) => format!("string:{r}:{tick_type}:{secs}"),
            MdOut::Tick(r, MdTick::Generic { tick_type, value }) => format!("generic:{r}:{tick_type}:{value}"),
            MdOut::Tick(r, MdTick::Option(c)) => format!("option:{r}:{}:{}:{}", c.tick_type, c.implied_vol, c.opt_price),
            MdOut::MarketDataType(r, t) => format!("mdt:{r}:{t}"),
            MdOut::SnapshotEnd(r) => format!("end:{r}"),
        }).collect()
    }

    fn poll(core: &ClientCore, shared: &SharedState) -> Vec<String> {
        let mut out = Vec::new();
        core.poll_market_data(shared, None, &mut out);
        lines(&out)
    }

    /// A book update of instrument `id` with this bid (ask 101, sizes 1).
    fn book(shared: &SharedState, id: InstrumentId, bid: i64) {
        let q = Quote { bid: bid * PRICE_SCALE, ask: 101 * PRICE_SCALE, bid_size: QTY_SCALE, ask_size: QTY_SCALE, ..Default::default() };
        shared.market.push_test_message(id, &q, &TestMessage { steps: Some(&[MdStep::Quote]), ..Default::default() });
    }

    fn stream(core: &ClientCore, shared: &SharedState, req_id: i64, instrument: InstrumentId) {
        assert!(core.attach_md_request(shared, req_id, instrument, false, "STK", "SMART", None, false, false));
    }

    // ibx#446: the reference sent a request's ticks before its cancel: a
    // client that reads after the cancel still gets them, and nothing of
    // the steps after.
    #[test]
    fn a_cancelled_request_gets_the_steps_before_its_cancel() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        stream(&core, &shared, 1, 5);
        book(&shared, 5, 100);
        book(&shared, 5, 99);
        assert!(core.unregister_mkt_data(&shared, 1).is_some());
        assert!(!shared.market.md_events.listened(5), "no step of the contract is written any more");
        book(&shared, 5, 98);
        assert_eq!(poll(&core, &shared), [
            "mdt:1:1", "price:1:1:100", "size:1:0:1", "price:1:2:101", "size:1:3:1", "size:1:0:1", "size:1:3:1",
            "price:1:1:99", "size:1:0:1",
        ]);
        assert!(poll(&core, &shared).is_empty());
    }

    // ibx#681, ibx#647 (the reference 10.51, 13/10/2026, an option with
    // no trade): its snapshot gives a volume of 0 and no halted state.
    #[test]
    fn an_option_snapshot_gives_a_zero_volume_and_no_halted_state() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        assert!(core.attach_md_request(&shared, 1, 5, true, "OPT", "SMART", None, false, false));
        let q = Quote { bid: 245 * PRICE_SCALE, ask: 246 * PRICE_SCALE, bid_size: QTY_SCALE, ask_size: QTY_SCALE, ..Default::default() };
        shared.market.push_test_message(5, &q, &TestMessage { steps: Some(&[MdStep::Quote, MdStep::Last, MdStep::Daily]), halted: Some(0),
            sizes_seen: true, ..Default::default() });
        let lines = poll(&core, &shared);
        assert!(lines.iter().any(|l| l == "size:1:8:0"), "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("generic:1:49")), "{lines:?}");
    }

    // ibx#680 (the reference 10.51, SPX on CBOE, 13/10/2026, regular
    // hours): an index snapshot gives a time of 0, high, low and close
    // unset, the bid and ask with their sizes, and nothing of the last; it
    // is not complete (it ends at the time limit). ibx#676: a snapshot asked
    // with no type takes its definition's.
    #[test]
    fn an_index_snapshot_and_a_retyped_one() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        assert!(core.attach_md_request(&shared, 1, 5, true, "IND", "CBOE", None, false, false));
        let q = Quote { bid: 7800 * PRICE_SCALE, ask: 7801 * PRICE_SCALE, bid_size: QTY_SCALE, ask_size: QTY_SCALE,
            last: 7800 * PRICE_SCALE, high: 7810 * PRICE_SCALE, ..Default::default() };
        shared.market.push_test_message(5, &q, &TestMessage { steps: Some(&[MdStep::Quote, MdStep::Last]), ..Default::default() });
        let max = f64::MAX;
        assert_eq!(poll(&core, &shared), ["mdt:1:1".to_string(), "string:1:45:0".into(), format!("price:1:6:{max}"),
            format!("price:1:7:{max}"), format!("price:1:9:{max}"), "price:1:1:7800".into(), "size:1:0:1".into(),
            "price:1:2:7801".into(), "size:1:3:1".into()]);
        assert!(!core.snapshot_reqs.lock().unwrap()[&1].complete());

        let mut snap = PlainSnapshot::new("", std::time::Instant::now());
        assert!(snap.untyped);
        snap.retype("OPT");
        assert!(!snap.untyped && !snap.index);
        snap.retype("IND");
        assert!(!snap.index, "typed once");
    }

    // ibx#671 (the reference 10.51, 13/10/2026, an option with no trade
    // today): a last of -1 with no size gives no last price, its size once.
    #[test]
    fn a_last_of_minus_one_without_size_is_not_given() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        assert!(core.attach_md_request(&shared, 1, 5, false, "OPT", "SMART", None, false, false));
        let q = Quote { last: -PRICE_SCALE, ..Default::default() };
        shared.market.push_test_message(5, &q, &TestMessage { steps: Some(&[MdStep::Last]), sizes_seen: true, ..Default::default() });
        assert_eq!(poll(&core, &shared), ["mdt:1:1", "size:1:5:0"]);
    }

    // ibx#640 (the reference 10.51, SPX on CBOE, 13/10/2026, regular
    // hours): an index stream gives bid and ask unset with unset sizes,
    // its last with an unset size, and no size or volume of its own.
    #[test]
    fn an_index_stream_has_no_bid_ask_or_sizes() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        assert!(core.attach_md_request(&shared, 1, 5, false, "IND", "CBOE", None, false, false));
        let q = Quote { bid: 7800 * PRICE_SCALE, ask: 7801 * PRICE_SCALE, bid_size: QTY_SCALE, ask_size: QTY_SCALE,
            last: 7800 * PRICE_SCALE, last_size: QTY_SCALE, volume: 10 * QTY_SCALE, ..Default::default() };
        shared.market.push_test_message(5, &q, &TestMessage { steps: Some(&[MdStep::Quote, MdStep::Last]), ..Default::default() });
        let max = f64::MAX;
        assert_eq!(poll(&core, &shared), ["mdt:1:1".to_string(), "price:1:4:7800".into(), format!("size:1:5:{max}"),
            format!("price:1:1:{max}"), format!("size:1:0:{max}"), format!("price:1:2:{max}"), format!("size:1:3:{max}")]);
    }

    // ibx#495 (the reference 10.51, paper 12/10/2026, MSFT with generic
    // ticks 293 to 295): when a request joins a streaming contract, the
    // running request gets all of the quote again, then the one that joins,
    // each with the generic values it asked that are known.
    #[test]
    fn a_join_gives_the_picture_to_every_request_with_the_generic_values() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        let asked = || MdGeneric { codes: vec![293, 294], exchange: "SMART".into(), sec_type: "STK".into() };
        core.md_generic.lock().unwrap().insert(1, asked());
        stream(&core, &shared, 1, 5);
        book(&shared, 5, 100);
        shared.market.push_generic_ticks(crate::bridge::GenericTicks {
            at: shared.market.md_events.position(), instrument: 5, code: 293,
            ticks: vec![crate::control::generic_values::GenTick::Generic(54, 78096.0)],
        });
        assert_eq!(poll(&core, &shared).last().map(String::as_str), Some("generic:1:54:78096"));

        core.md_generic.lock().unwrap().insert(2, asked());
        stream(&core, &shared, 2, 5);
        book(&shared, 5, 99);
        let lines = poll(&core, &shared);
        let picture = |r: i64| vec![
            format!("price:{r}:1:100"), format!("size:{r}:0:1"), format!("price:{r}:2:101"), format!("size:{r}:3:1"),
            format!("size:{r}:0:1"), format!("size:{r}:3:1"), format!("generic:{r}:54:78096"),
        ];
        let mut want: Vec<String> = picture(1);
        want.push("mdt:2:1".into());
        want.extend(picture(2));
        assert_eq!(lines[..want.len()], want[..], "{lines:?}");
    }

    fn computation(tick_type: i32, iv: f64) -> OptionComputation {
        OptionComputation {
            req_id: 0, tick_type, tick_attrib: 0, implied_vol: iv, delta: crate::control::optcalc::NOT_COMPUTED,
            opt_price: 14.949999809265137, pv_dividend: 0.2690576206220461, gamma: crate::control::optcalc::NOT_COMPUTED,
            vega: crate::control::optcalc::NOT_COMPUTED, theta: crate::control::optcalc::NOT_COMPUTED, und_price: 339.585,
        }
    }

    fn push_computations(shared: &SharedState, id: InstrumentId, comps: &[OptionComputation]) {
        shared.market.push_generic_ticks(crate::bridge::GenericTicks {
            at: shared.market.md_events.position(), instrument: id, code: market::COMPUTATIONS,
            ticks: comps.iter().map(|c| crate::control::generic_values::GenTick::Option(*c)).collect(),
        });
    }

    const NO: f64 = crate::control::optcalc::NOT_COMPUTED;

    // ibx#502: a stream gets every computation of its option's model, after
    // its market data type.
    #[test]
    fn an_option_stream_gets_its_computations() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        assert!(core.attach_md_request(&shared, 1, 5, false, "OPT", "SMART", None, false, false));
        push_computations(&shared, 5, &[computation(13, 0.24), computation(10, NO)]);
        assert_eq!(poll(&core, &shared), ["mdt:1:1", "option:1:13:0.24:14.949999809265137",
            format!("option:1:10:{NO}:14.949999809265137").as_str()]);
    }

    // ibx#502 (the reference 10.51, 12/10/2026): a plain snapshot of an
    // option takes each computation once it has a volatility or a greek
    // (the model's at once) and ends once its quote and the four came; at
    // its time limit it first gets those it did not take, as known.
    #[test]
    fn an_option_snapshot_ends_on_its_computations_or_at_its_limit() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        let q = Quote { bid: 100 * PRICE_SCALE, ask: 101 * PRICE_SCALE, last: 100 * PRICE_SCALE, close: 99 * PRICE_SCALE,
            bid_size: QTY_SCALE, ask_size: QTY_SCALE, last_size: QTY_SCALE, ..Default::default() };
        for req in [1, 2] {
            let id = 4 + req as InstrumentId;
            assert!(core.attach_md_request(&shared, req, id, true, "OPT", "SMART", None, false, false));
            shared.market.push_test_message(id, &q, &TestMessage::default());
        }
        // Request 1: the four, valid: it ends.
        push_computations(&shared, 5, &[computation(13, 0.24), computation(10, 0.23), computation(11, 0.25), computation(12, 0.24)]);
        // Request 2: tick 10 has no volatility yet: not taken.
        push_computations(&shared, 6, &[computation(13, 0.24), computation(10, NO), computation(11, 0.25), computation(12, 0.24)]);
        let lines = poll(&core, &shared);
        let options: Vec<&String> = lines.iter().filter(|l| l.starts_with("option:") || l.starts_with("end:")).collect();
        assert_eq!(options, ["option:1:13:0.24:14.949999809265137", "option:1:10:0.23:14.949999809265137",
            "option:1:11:0.25:14.949999809265137", "option:1:12:0.24:14.949999809265137", "end:1",
            "option:2:13:0.24:14.949999809265137", "option:2:11:0.25:14.949999809265137", "option:2:12:0.24:14.949999809265137"],
            "{lines:?}");
        let mut out = Vec::new();
        core.poll_market_data(&shared, Some(std::time::Instant::now() + crate::control::snapshot::TIMEOUT), &mut out);
        assert_eq!(lines_of(&out), [format!("option:2:10:{NO}:14.949999809265137"), "end:2".to_string()]);
    }

    fn lines_of(out: &[MdOut]) -> Vec<String> {
        lines(out)
    }

    // ibx#446: a slot the engine gives to another contract: the new
    // request gets only the new contract's steps, from an empty record;
    // the earlier contract's steps go to its cancelled request.
    #[test]
    fn a_reused_slot_starts_empty_after_the_earlier_steps() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        stream(&core, &shared, 1, 5);
        let q = Quote { bid: 100 * PRICE_SCALE, high: 120 * PRICE_SCALE, ..Default::default() };
        shared.market.push_test_message(5, &q, &TestMessage::default());
        core.unregister_mkt_data(&shared, 1);
        stream(&core, &shared, 2, 5);
        book(&shared, 5, 50);
        assert_eq!(poll(&core, &shared), [
            "mdt:1:1", "price:1:6:120", "price:1:1:100", "size:1:0:0",
            "mdt:2:1", "price:2:1:50", "size:2:0:1", "price:2:2:101", "size:2:3:1", "size:2:0:1", "size:2:3:1",
        ]);
    }

    // ibx#446: a request that takes over the P&L quote (data, but no step
    // queued) waits for the engine's catch-up, the quote as the engine has
    // it, and gets all of it there in one step; the steps written before
    // the catch-up are not its own.
    #[test]
    fn a_request_on_the_pnl_quote_starts_from_a_catch_up() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        let (tx, _rx) = crossbeam_channel::unbounded();
        let tx = crate::engine::park::ControlSender::from(tx);
        core.pnl_quotes.lock().unwrap().active.insert(756733, 5);
        core.register_mkt_data(&shared, &tx, 1, 756733, "SPY", "SMART", "STK", "USD", &Default::default(), false, "", 0)
            .unwrap();
        let queue = &shared.market.md_events;
        assert!(queue.listened(5) && queue.needs_catch_up(5));
        book(&shared, 5, 99);
        assert!(poll(&core, &shared).is_empty(), "waits for the catch-up");
        let q = Quote { bid: 100 * PRICE_SCALE, last: 100 * PRICE_SCALE, last_size: 2 * QTY_SCALE, ..Default::default() };
        let mut marks = QuoteMarks::default();
        marks.set_halted(0);
        queue.write_catch_ups(|id| MdEvent::catch_up(id, &q, marks));
        assert!(!queue.needs_catch_up(5));
        book(&shared, 5, 101);
        assert_eq!(poll(&core, &shared), [
            "mdt:1:1", "price:1:1:100", "size:1:0:0", "price:1:4:100", "size:1:5:2", "size:1:5:2", "generic:1:49:0",
            "price:1:1:101", "size:1:0:1", "price:1:2:101", "size:1:3:1", "size:1:0:1", "size:1:3:1", "generic:1:49:0",
        ]);
    }

    // ibx#444, ibx#446: a request without a conId joins the subscription of
    // the contract the engine found at the moment it found it: the quote as
    // it was then, then the later steps, even when the client reads later.
    // A step goes to the newest request first (captured 05/10/2026).
    #[test]
    fn a_symbol_only_request_joins_where_the_engine_found_its_contract() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        stream(&core, &shared, 1, 5);
        stream(&core, &shared, 2, 6);
        book(&shared, 5, 100);
        shared.market.push_md_merge(6, 5);
        book(&shared, 5, 99);
        let (notices, _) = core.take_md_rejects(&shared);
        assert!(notices.is_empty(), "{notices:?}");
        // The running request gets the quote again when the other joins
        // (the reference 10.51, paper 12/10/2026, ibx#495).
        assert_eq!(poll(&core, &shared), [
            "mdt:1:1", "price:1:1:100", "size:1:0:1", "price:1:2:101", "size:1:3:1", "size:1:0:1", "size:1:3:1",
            "price:1:1:100", "size:1:0:1", "price:1:2:101", "size:1:3:1", "size:1:0:1", "size:1:3:1",
            "mdt:2:1", "price:2:1:100", "size:2:0:1", "price:2:2:101", "size:2:3:1", "size:2:0:1", "size:2:3:1",
            "price:2:1:99", "size:2:0:1", "price:1:1:99", "size:1:0:1",
        ]);
    }

    // ibx#444, captured 07/10/2026: on a paper session a streaming request
    // ended by a refused news tick gets 10197 5 s later, unless a request
    // of its contract runs then. A running request gets none.
    #[test]
    fn a_refused_news_request_gets_10197_unless_its_contract_runs() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        let no_data = |after: std::time::Duration| -> Vec<i64> {
            core.take_md_no_data(std::time::Instant::now() + after).into_iter().map(|n| match n {
                MdNotice::Error { req_id, code: 10197, text } if text == "No market data during competing live session" => req_id,
                other => panic!("{other:?}"),
            }).collect()
        };
        let late = MD_NO_DATA_WAIT + std::time::Duration::from_secs(1);
        core.note_news_refused(&shared, 1, 600, std::time::Instant::now());
        assert!(no_data(late).is_empty(), "a live session");

        shared.reference.set_paper_session(true);
        stream(&core, &shared, 2, 7);
        core.con_id_to_instrument.lock().unwrap().extend([(700, 7), (600, 6)]);
        core.note_news_refused(&shared, 10, 700, std::time::Instant::now());
        core.note_news_refused(&shared, 11, 600, std::time::Instant::now());
        assert!(no_data(std::time::Duration::from_secs(4)).is_empty(), "before the wait ends");
        assert_eq!(no_data(late), [11]);
        assert!(no_data(2 * late).is_empty(), "once; none for the running request");

        // Through the engine (a request without a conId): the request ends,
        // then 10197.
        stream(&core, &shared, 3, 8);
        shared.market.push_md_reject(crate::bridge::MdReject::NewsRefused { instrument: 8, con_id: 800, text: "x".into() });
        let (notices, _) = core.take_md_rejects(&shared);
        assert!(matches!(notices[..], [MdNotice::Error { req_id: 3, code: 10094, .. }]), "{notices:?}");
        assert_eq!(no_data(late), [3]);

        // The contract runs on the slot of a request sent without a conId.
        stream(&core, &shared, 4, 9);
        shared.market.push_md_resolved(9, 900);
        let _ = core.take_md_rejects(&shared);
        core.note_news_refused(&shared, 12, 900, std::time::Instant::now());
        assert!(no_data(late).is_empty());
    }

    // ibx#501, captured 07/10/2026 (ib-agent#202): the 5 s count from the
    // request, not from the refusal; two refused requests of one contract
    // within a pass of the line manager get one 10197, 0.1 s apart two.
    #[test]
    fn the_10197_wait_counts_from_the_request_and_once_per_pass() {
        let (core, shared) = (ClientCore::new(), SharedState::new());
        shared.reference.set_paper_session(true);
        let no_data = |at: std::time::Instant| -> Vec<i64> {
            core.take_md_no_data(at).into_iter().map(|n| match n {
                MdNotice::Error { req_id, code: 10197, .. } => req_id,
                other => panic!("{other:?}"),
            }).collect()
        };
        let ms = std::time::Duration::from_millis;
        let t0 = std::time::Instant::now();
        // Refused 130 ms after its request (contract looked up first).
        core.note_news_refused(&shared, 1, 600, t0);
        assert!(no_data(t0 + MD_NO_DATA_WAIT - ms(1)).is_empty());
        assert_eq!(no_data(t0 + MD_NO_DATA_WAIT), [1]);

        core.note_news_refused(&shared, 2, 700, t0);
        core.note_news_refused(&shared, 3, 700, t0 + ms(2));
        core.note_news_refused(&shared, 4, 700, t0 + ms(100));
        // A request asked earlier but refused later goes first.
        core.note_news_refused(&shared, 5, 800, t0 - ms(50));
        assert_eq!(no_data(t0 + MD_NO_DATA_WAIT + ms(200)), [5, 2, 4]);
    }
}
