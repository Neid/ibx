//! The market data steps of each farm message, queued from the engine to
//! the API client (ibx#446).
//!
//! The reference sends the ticks of a market data request while it decodes
//! each server message, one step at a time, with that message's values:
//! the daily figures and each trade (its time, its exchange, then its price
//! and size) in the order of the message, then each book update.
//! Its API socket buffers what the client has not read yet, so a client
//! that reads slower than the messages come still gets every step, never
//! two messages merged into one.
//!
//! ibx's engine writes each step as an [`MdEvent`] into [`MdQueue`], a ring
//! the API client's dispatch reads: one writer (the engine's thread), one
//! reader (the dispatch, under the client's lock), fixed size, no lock and
//! no allocation. A message's steps are written as its blocks are read and
//! handed over together at its end; the first one is marked, so that the
//! reader takes the book updates after the message's other steps, as the
//! reference applies them. The quote snapshot of the engine API
//! (`MarketDataState::quote`) is kept as before, apart from this queue.
//!
//! A full ring: the reference's buffer has no limit, but ibx's engine loop
//! can neither wait for the client nor grow without bound. The steps of an
//! instrument that do not fit are dropped, and as soon as the ring is at
//! most half full the engine writes one catch-up step with the instrument's
//! whole quote (`MdStep::CatchUp`): the client sends what changed since its
//! last step, in one step. The intermediate values of that instrument are
//! lost; its last values are not, and nothing else is.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::types::{InstrumentId, Quote, QuoteMarks, MAX_INSTRUMENTS};

/// The quote fields a step carries, as indexes of [`MdEvent::values`].
pub mod field {
    pub const BID: usize = 0;
    pub const ASK: usize = 1;
    pub const LAST: usize = 2;
    pub const BID_SIZE: usize = 3;
    pub const ASK_SIZE: usize = 4;
    pub const LAST_SIZE: usize = 5;
    pub const HIGH: usize = 6;
    pub const LOW: usize = 7;
    pub const VOLUME: usize = 8;
    pub const CLOSE: usize = 9;
    pub const OPEN: usize = 10;
    /// Last trade time, ns since the epoch.
    pub const TIME: usize = 11;
    pub const BID_EXCH: usize = 12;
    pub const ASK_EXCH: usize = 13;
    pub const LAST_EXCH: usize = 14;
    /// Number of fields.
    pub const COUNT: usize = 15;
    /// Every field.
    pub const ALL: u16 = (1 << COUNT) - 1;
    /// The fields of a book update.
    pub const QUOTE: u16 = 1 << BID | 1 << ASK | 1 << BID_SIZE | 1 << ASK_SIZE | 1 << BID_EXCH | 1 << ASK_EXCH;
    /// The fields of a trade's last price step.
    pub const LAST_TRADE: u16 = 1 << LAST | 1 << LAST_SIZE;
    /// The fields of a daily figures update.
    pub const DAILY: u16 = 1 << HIGH | 1 << LOW | 1 << VOLUME | 1 << CLOSE | 1 << OPEN;
}

/// The fields of a quote, in the order of [`field`].
#[inline(always)]
pub fn quote_values(q: &Quote) -> [i64; field::COUNT] {
    [
        q.bid, q.ask, q.last, q.bid_size, q.ask_size, q.last_size,
        q.high, q.low, q.volume, q.close, q.open, q.timestamp_ns as i64,
        q.bid_exch_mask, q.ask_exch_mask, q.last_exch_mask,
    ]
}

/// One field of a quote, by its index in [`field`].
#[inline(always)]
pub fn quote_field(q: &Quote, idx: usize) -> i64 {
    match idx {
        field::BID => q.bid,
        field::ASK => q.ask,
        field::LAST => q.last,
        field::BID_SIZE => q.bid_size,
        field::ASK_SIZE => q.ask_size,
        field::LAST_SIZE => q.last_size,
        field::HIGH => q.high,
        field::LOW => q.low,
        field::VOLUME => q.volume,
        field::CLOSE => q.close,
        field::OPEN => q.open,
        field::TIME => q.timestamp_ns as i64,
        field::BID_EXCH => q.bid_exch_mask,
        field::ASK_EXCH => q.ask_exch_mask,
        _ => q.last_exch_mask,
    }
}

/// The header word of a step in the ring.
#[inline(always)]
fn header_word(instrument: InstrumentId, step: MdStep, fields: u16, halted: Option<u8>, auto: u8, seen: u8, first: bool) -> u64 {
    (instrument as u64 & 0xFFFF)
        | (step as u64) << 16
        | (halted.map_or(0xFF, |h| h & 3) as u64) << 24
        | (auto as u64) << 32
        | (seen as u64) << 40
        | ((fields & field::ALL) as u64) << 48
        | (first as u64) << 63
}

/// The field a wire tick of a binary tick message block sets, as `MarketState::apply_tick`
/// applies it: `trade` for a trade stream tag, `stats` for a daily-stats
/// block. None for a type that sets no quote field.
#[inline(always)]
pub fn wire_field(tick_type: u64, magnitude: i64, stats: bool) -> Option<usize> {
    use crate::protocol::tick_decoder as td;
    Some(match tick_type {
        td::O_BID_PRICE => field::BID,
        td::O_ASK_PRICE => field::ASK,
        td::O_LAST_PRICE => field::LAST,
        td::O_CLOSE_PRICE => field::CLOSE,
        td::O_HIGH_PRICE => field::HIGH,
        td::O_LOW_PRICE => field::LOW,
        td::O_OPEN_PRICE => field::OPEN,
        td::O_BID_SIZE => field::BID_SIZE,
        td::O_ASK_SIZE => field::ASK_SIZE,
        td::O_LAST_SIZE => field::LAST_SIZE,
        td::O_VOLUME => field::VOLUME,
        td::O_BID_EXCH => field::BID_EXCH,
        td::O_ASK_EXCH => field::ASK_EXCH,
        td::O_LAST_EXCH => field::LAST_EXCH,
        // On a daily-stats block the base is the close date, not a time.
        td::O_TIMESTAMP_BASE if !stats && magnitude > 0 => field::TIME,
        td::O_TIMESTAMP_DELTA if magnitude > 0 => field::TIME,
        _ => return None,
    })
}

/// One step of a farm message, as the reference notifies its API
/// subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MdStep {
    /// The daily figures of a daily-stats block.
    Daily = 1,
    /// A trade's time.
    Time = 2,
    /// A trade's exchange.
    Exchange = 3,
    /// A trade's price and size (its status too); every trade has one.
    Last = 4,
    /// A book update, after the trades and daily figures of its message.
    Quote = 5,
    /// The instrument's whole quote after steps were lost (a full queue)
    /// or for a request that takes over a quote with data: the
    /// differences go out as one step.
    CatchUp = 6,
    /// What a contract kept from its earlier requests and that a new
    /// request does not get again (ibx#508): the last trade's exchange.
    /// Sets the request's baseline, sends nothing.
    Kept = 7,
    /// The bid and ask a contract kept, given to a new request as a book
    /// update; a snapshot gives them without auto-execution (ibx#671).
    KeptQuote = 8,
    /// The trade an option's frozen view repeats (ibx#647): a stream gives
    /// its last size and time again; a snapshot nothing.
    Again = 9,
}

impl MdStep {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Daily,
            2 => Self::Time,
            3 => Self::Exchange,
            4 => Self::Last,
            5 => Self::Quote,
            7 => Self::Kept,
            8 => Self::KeptQuote,
            9 => Self::Again,
            _ => Self::CatchUp,
        }
    }
}

/// A queued step: which fields it sets and their values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MdEvent {
    pub instrument: InstrumentId,
    pub step: MdStep,
    /// The fields the step sets (bit i: field i of [`field`]); the other
    /// values are not of this step (0 once read from the queue).
    pub fields: u16,
    /// The trade's status (its two low bits: 1 halted, 2 volatility
    /// halted) on a last price step; the last known one on a catch-up.
    pub halted: Option<u8>,
    /// Whether the bid and the ask can execute automatically, as the farm
    /// said, in the bits of `QuoteMarks` (0: not said): on a book update and
    /// a catch-up.
    pub auto: u8,
    /// The sizes the farm gave at least once (`SizeKind` bits): on a
    /// catch-up; on the other steps the sizes set are given.
    pub seen: u8,
    /// The first step of its message (set by the queue when written).
    pub first: bool,
    pub values: [i64; field::COUNT],
}

impl MdEvent {
    /// A step of an instrument that sets nothing yet.
    pub fn new(instrument: InstrumentId, step: MdStep) -> Self {
        Self { instrument, step, fields: 0, halted: None, auto: 0, seen: 0, first: true, values: [0; field::COUNT] }
    }

    /// The step sets `field` to `value`.
    pub fn with(mut self, field: usize, value: i64) -> Self {
        self.fields |= 1 << field;
        self.values[field] = value;
        self
    }

    /// The step sets the fields `mask` to the quote's values.
    pub fn with_quote(mut self, q: &Quote, mask: u16) -> Self {
        self.values = quote_values(q);
        self.fields |= mask;
        self
    }

    /// The whole quote of an instrument and what the farm told of it.
    pub fn catch_up(instrument: InstrumentId, q: &Quote, marks: QuoteMarks) -> Self {
        let mut ev = Self::new(instrument, MdStep::CatchUp).with_quote(q, field::ALL);
        ev.halted = marks.halted().map(|s| s as u8);
        ev.auto = marks.auto_word();
        ev.seen = marks.seen_word();
        ev
    }

    fn header(&self, first: bool) -> u64 {
        header_word(self.instrument, self.step, self.fields, self.halted, self.auto, self.seen, first)
    }

    fn from_header(h: u64, values: [i64; field::COUNT]) -> Self {
        let halted = ((h >> 24) & 0xFF) as u8;
        Self {
            instrument: (h & 0xFFFF) as InstrumentId,
            step: MdStep::from_u8(((h >> 16) & 0xFF) as u8),
            halted: (halted != 0xFF).then_some(halted),
            auto: ((h >> 32) & 0xFF) as u8,
            seen: ((h >> 40) & 0xFF) as u8,
            fields: ((h >> 48) as u16) & field::ALL,
            first: h >> 63 != 0,
            values,
        }
    }
}

/// Steps the ring holds: 2 MB, a few seconds of a busy session even for a
/// client that stops reading.
pub const MD_QUEUE_CAPACITY: u64 = 1 << 12;
const MASK: u64 = MD_QUEUE_CAPACITY - 1;
const WORDS: usize = MAX_INSTRUMENTS.div_ceil(64);

/// One step in the ring: its header and its values, as atomic words, so
/// that neither side needs `unsafe`; with `Relaxed` these are plain loads
/// and stores.
#[repr(C, align(64))]
struct Slot([AtomicU64; 1 + field::COUNT]);

/// A word on its own cache line.
#[repr(C, align(64))]
#[derive(Default)]
struct Line(AtomicU64);

/// The engine's side: its next slot, the last reader position it saw, and
/// whether the next step starts a message.
#[repr(C, align(64))]
struct Writer {
    next: AtomicU64,
    tail_seen: AtomicU64,
    starting: AtomicBool,
}

impl Default for Writer {
    fn default() -> Self {
        Self { next: AtomicU64::new(0), tail_seen: AtomicU64::new(0), starting: AtomicBool::new(true) }
    }
}

/// The ring of steps from the engine to the API client (see the module).
pub struct MdQueue {
    slots: Box<[Slot]>,
    writer: Writer,
    /// Steps written and handed to the reader.
    head: Line,
    /// Steps the reader is done with.
    tail: Line,
    /// Instruments with an API market data request: the only ones whose
    /// steps are written.
    listened: [AtomicU64; WORDS],
    /// Instruments whose next step is a catch-up: steps were lost, or a
    /// request asked for the whole quote.
    resync: [AtomicU64; WORDS],
    resync_any: AtomicBool,
}

impl Default for MdQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
fn bit(id: InstrumentId) -> (usize, u64) {
    ((id as usize >> 6) % WORDS, 1u64 << (id & 63))
}

impl MdQueue {
    pub fn new() -> Self {
        Self {
            slots: (0..MD_QUEUE_CAPACITY).map(|_| Slot(std::array::from_fn(|_| AtomicU64::new(0)))).collect(),
            writer: Writer::default(),
            head: Line::default(),
            tail: Line::default(),
            listened: std::array::from_fn(|_| AtomicU64::new(0)),
            resync: std::array::from_fn(|_| AtomicU64::new(0)),
            resync_any: AtomicBool::new(false),
        }
    }

    // ── Engine side (one thread) ──

    /// Whether an API request listens to the instrument.
    #[inline(always)]
    pub fn listened(&self, id: InstrumentId) -> bool {
        let (w, b) = bit(id);
        self.listened[w].load(Ordering::Acquire) & b != 0
    }

    /// Whether the instrument waits for a catch-up: its steps are not
    /// written meanwhile.
    #[inline(always)]
    pub fn needs_catch_up(&self, id: InstrumentId) -> bool {
        let (w, b) = bit(id);
        self.resync[w].load(Ordering::Acquire) & b != 0
    }

    /// Write a step of a listened instrument (not handed to the reader
    /// before `publish`). A step that does not fit is dropped and the
    /// instrument waits for a catch-up; false when not written.
    #[inline]
    pub fn offer(&self, ev: &MdEvent) -> bool {
        if !self.listened(ev.instrument) || self.needs_catch_up(ev.instrument) {
            return false;
        }
        self.push(ev) || self.full(ev.instrument)
    }

    /// `offer` of a step whose values are the quote's (the engine's own
    /// quote, with no copy of it): the step `step` of the instrument sets
    /// the fields `fields`, with the trade's status `halted` and the book's
    /// auto-execution bits `auto`.
    #[inline]
    pub fn offer_quote(
        &self, instrument: InstrumentId, step: MdStep, fields: u16, q: &Quote, halted: Option<u8>, auto: u8,
    ) -> bool {
        if !self.listened(instrument) || self.needs_catch_up(instrument) {
            return false;
        }
        self.push_with(|first| header_word(instrument, step, fields, halted, auto, 0, first), fields, |i| quote_field(q, i))
            || self.full(instrument)
    }

    /// A step that did not fit: the instrument waits for a catch-up.
    #[cold]
    fn full(&self, instrument: InstrumentId) -> bool {
        log::warn!("Market data queue full: instrument {} skips steps until the client reads (ibx#446)", instrument);
        self.want_catch_up(instrument);
        false
    }

    /// Write a step; false when the ring is full.
    #[inline]
    fn push(&self, ev: &MdEvent) -> bool {
        self.push_with(|first| ev.header(first), ev.fields, |i| ev.values[i])
    }

    /// Write a step from its header and the values of its fields.
    #[inline(always)]
    fn push_with(&self, header: impl FnOnce(bool) -> u64, fields: u16, value: impl Fn(usize) -> i64) -> bool {
        let next = self.writer.next.load(Ordering::Relaxed);
        if next.wrapping_sub(self.writer.tail_seen.load(Ordering::Relaxed)) >= MD_QUEUE_CAPACITY {
            let tail = self.tail.0.load(Ordering::Acquire);
            self.writer.tail_seen.store(tail, Ordering::Relaxed);
            if next.wrapping_sub(tail) >= MD_QUEUE_CAPACITY {
                return false;
            }
        }
        let slot = &self.slots[(next & MASK) as usize].0;
        // One writer: a load and a store, no locked exchange.
        let first = self.writer.starting.load(Ordering::Relaxed);
        if first {
            self.writer.starting.store(false, Ordering::Relaxed);
        }
        slot[0].store(header(first), Ordering::Relaxed);
        // Only the fields of the step, packed in their order: a step of 7
        // fields or fewer stays on the header's cache line.
        let mut set = fields & field::ALL;
        let mut w = 1;
        while set != 0 {
            let i = set.trailing_zeros() as usize;
            set &= set - 1;
            slot[w].store(value(i) as u64, Ordering::Relaxed);
            w += 1;
        }
        self.writer.next.store(next + 1, Ordering::Relaxed);
        true
    }

    /// The position of the next step the engine writes.
    #[inline(always)]
    pub fn position(&self) -> u64 {
        self.writer.next.load(Ordering::Relaxed)
    }

    /// Hand the steps written to the reader: the end of a message.
    #[inline]
    pub fn publish(&self) {
        let next = self.writer.next.load(Ordering::Relaxed);
        if self.head.0.load(Ordering::Relaxed) != next {
            self.head.0.store(next, Ordering::Release);
            self.writer.starting.store(true, Ordering::Relaxed);
        }
    }

    /// Whether an instrument waits for a catch-up.
    #[inline(always)]
    pub fn catch_up_wanted(&self) -> bool {
        self.resync_any.load(Ordering::Relaxed)
    }

    /// Write the catch-ups wanted, `make` giving each instrument's step,
    /// while the ring is at most half full; the others wait.
    pub fn write_catch_ups(&self, mut make: impl FnMut(InstrumentId) -> MdEvent) {
        if !self.resync_any.swap(false, Ordering::AcqRel) {
            return;
        }
        for (w, word) in self.resync.iter().enumerate() {
            let mut ids = word.swap(0, Ordering::AcqRel);
            while ids != 0 {
                let id = (w * 64) as InstrumentId + ids.trailing_zeros();
                ids &= ids - 1;
                let used = self.writer.next.load(Ordering::Relaxed).wrapping_sub(self.tail.0.load(Ordering::Acquire));
                if !self.listened(id) {
                    continue;
                }
                if used > MD_QUEUE_CAPACITY / 2 || !self.push(&make(id)) {
                    self.want_catch_up(id);
                }
            }
        }
        self.publish();
    }

    fn want_catch_up(&self, id: InstrumentId) {
        let (w, b) = bit(id);
        self.resync[w].fetch_or(b, Ordering::AcqRel);
        self.resync_any.store(true, Ordering::Release);
    }

    // ── Reader side (one at a time) ──

    /// Start or stop writing the steps of an instrument.
    pub fn listen(&self, id: InstrumentId, on: bool) {
        let (w, b) = bit(id);
        if on {
            self.listened[w].fetch_or(b, Ordering::AcqRel);
        } else {
            self.listened[w].fetch_and(!b, Ordering::AcqRel);
        }
    }

    /// Ask the engine for a catch-up of the instrument.
    pub fn request_catch_up(&self, id: InstrumentId) {
        self.want_catch_up(id);
    }

    /// The position after the last step handed to the reader.
    #[inline(always)]
    pub fn head(&self) -> u64 {
        self.head.0.load(Ordering::Acquire)
    }

    /// The position of the first step the reader has not read.
    #[inline(always)]
    pub fn tail(&self) -> u64 {
        self.tail.0.load(Ordering::Relaxed)
    }

    /// The step at `seq`, for `tail() <= seq < head()`; the values of the
    /// fields it does not set are 0.
    #[inline]
    pub fn read(&self, seq: u64) -> MdEvent {
        let slot = &self.slots[(seq & MASK) as usize].0;
        let mut ev = MdEvent::from_header(slot[0].load(Ordering::Relaxed), [0; field::COUNT]);
        let mut set = ev.fields;
        let mut w = 1;
        while set != 0 {
            let i = set.trailing_zeros() as usize;
            set &= set - 1;
            ev.values[i] = slot[w].load(Ordering::Relaxed) as i64;
            w += 1;
        }
        ev
    }

    /// The kind of the step at `seq`, and whether it starts its message.
    #[inline]
    pub fn step_at(&self, seq: u64) -> (MdStep, bool) {
        let h = self.slots[(seq & MASK) as usize].0[0].load(Ordering::Relaxed);
        (MdStep::from_u8(((h >> 16) & 0xFF) as u8), h >> 63 != 0)
    }

    /// The reader is done with the steps before `seq`.
    #[inline]
    pub fn release(&self, seq: u64) {
        self.tail.0.store(seq, Ordering::Release);
    }

    /// Write and hand over a step, as a message of its own, whoever
    /// listens (tests, as the engine).
    #[doc(hidden)]
    pub fn push_now(&self, ev: &MdEvent) -> bool {
        self.push_message(std::slice::from_ref(ev))
    }

    /// Write and hand over the steps of one message, whoever listens
    /// (tests, as the engine).
    #[doc(hidden)]
    pub fn push_message(&self, evs: &[MdEvent]) -> bool {
        let ok = evs.iter().all(|ev| self.push(ev));
        self.publish();
        ok
    }
}

/// The steps of one farm message for a quote, as a test writes them: each
/// step sets the fields of its group (`field::QUOTE`, `LAST_TRADE`,
/// `DAILY`, the time, the trade exchange) to the quote's values; with no
/// steps given, the trade's time, its price, the daily figures and the
/// book, each with the fields that are not 0. `sizes_seen`: every size of
/// a step is set, 0 too. The status goes with the last price step (or the
/// catch-up), the auto-execution bits with the book.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct TestMessage<'a> {
    pub steps: Option<&'a [MdStep]>,
    pub halted: Option<i64>,
    pub auto_bits: Option<i64>,
    pub sizes_seen: bool,
    /// The quote's pre-open state (ibx#692).
    pub pre_open: Option<bool>,
}

impl TestMessage<'_> {
    /// The steps of the message for the quote.
    pub fn events(&self, instrument: InstrumentId, q: &Quote) -> Vec<MdEvent> {
        use field::*;
        let values = quote_values(q);
        let group = |step: MdStep| -> u16 {
            match step {
                MdStep::Daily => DAILY,
                MdStep::Time => 1 << TIME,
                MdStep::Exchange | MdStep::Kept => 1 << LAST_EXCH,
                MdStep::Last => LAST_TRADE,
                MdStep::Again => 1 << LAST_SIZE | 1 << TIME,
                MdStep::Quote | MdStep::KeptQuote => QUOTE,
                MdStep::CatchUp => ALL,
            }
        };
        let sizes: u16 = 1 << BID_SIZE | 1 << ASK_SIZE | 1 << LAST_SIZE | 1 << VOLUME;
        let set = |mask: u16| -> u16 {
            (0..COUNT).filter(|&i| mask & (1 << i) != 0)
                .filter(|&i| values[i] != 0 || (self.sizes_seen && sizes & (1 << i) != 0))
                .fold(0, |m, i| m | 1 << i)
        };
        let default = [MdStep::Time, MdStep::Last, MdStep::Daily, MdStep::Quote];
        let mut out = Vec::new();
        let mut halted_given = false;
        for &step in self.steps.unwrap_or(&default) {
            let mut ev = MdEvent::new(instrument, step).with_quote(q, set(group(step)));
            let wanted = self.steps.is_some() || ev.fields != 0
                || (step == MdStep::Last && self.halted.is_some())
                || (step == MdStep::Quote && self.auto_bits.is_some());
            if !wanted {
                continue;
            }
            if matches!(step, MdStep::Last | MdStep::CatchUp) {
                ev.halted = self.halted.filter(|s| *s != -1).map(|s| (s & 3) as u8);
                halted_given |= ev.halted.is_some();
            }
            if matches!(step, MdStep::Quote | MdStep::KeptQuote | MdStep::CatchUp)
                && (self.auto_bits.is_some() || self.pre_open.is_some())
            {
                let mut marks = QuoteMarks::default();
                if let Some(bits) = self.auto_bits {
                    marks.set_auto_bits(bits);
                }
                if let Some(on) = self.pre_open {
                    marks.set_pre_open(on);
                }
                ev.auto = marks.auto_word();
            }
            out.push(ev);
        }
        // A status with no last price step: on a step of its own.
        if !halted_given && let Some(status) = self.halted.filter(|s| *s != -1) {
            let mut ev = MdEvent::new(instrument, MdStep::Last).with_quote(q, 0);
            ev.halted = Some((status & 3) as u8);
            out.push(ev);
        }
        out
    }
}

/// Powers of ten an [`Increment`] divides by.
const POW10: [f64; 16] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15,
];

/// A price or size increment of a contract's definition (ibx#613): a whole
/// number of units of ten to the minus `decimals`, so that a value rounded
/// to it is the nearest double of a decimal with those decimals (0.1 and
/// not 0.30000000000000004). Packed in one word for the engine to hand to
/// the API client per instrument, 0 being none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Increment {
    units: u64,
    decimals: u8,
}

impl Increment {
    /// The increment of a step such as 0.01 or 0.0001: None when it is not
    /// above 0, not finite, or not a decimal of at most 15 places.
    pub fn from_step(step: f64) -> Option<Self> {
        if !step.is_finite() || step <= 0.0 {
            return None;
        }
        for (decimals, p) in POW10.iter().enumerate() {
            let x = step * p;
            let r = x.round();
            if r >= 1.0 && r < (1u64 << 55) as f64 && (x - r).abs() <= 1e-9 * r {
                return Some(Self { units: r as u64, decimals: decimals as u8 });
            }
        }
        None
    }

    /// One word: the units, then the decimals in the low byte; never 0.
    pub fn pack(self) -> u64 {
        self.units << 8 | self.decimals as u64
    }

    /// The increment of a packed word; None for 0.
    #[inline(always)]
    pub fn unpack(word: u64) -> Option<Self> {
        let decimals = (word & 0xff) as usize;
        (word >> 8 != 0 && decimals < POW10.len()).then_some(Self { units: word >> 8, decimals: decimals as u8 })
    }

    /// `n` steps, as the nearest number to the exact decimal (ibx#613).
    #[inline]
    pub fn times(self, n: u64) -> f64 {
        (n as f64 * self.units as f64) / POW10[self.decimals as usize]
    }

    /// A value in the steps of the increment, the nearest step, with the
    /// increment's decimals. An unset value (`f64::MAX`) or one that is
    /// not finite stays as it is. No allocation, a few float operations.
    #[inline(always)]
    pub fn round(self, value: f64) -> f64 {
        if !value.is_finite() || value == f64::MAX {
            return value;
        }
        let (p, units) = (POW10[self.decimals as usize], self.units as f64);
        (value * p / units).round() * units / p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ibx#613: an increment rounds to its steps with its decimals.
    #[test]
    fn an_increment_rounds_to_its_steps_with_its_decimals() {
        let cent = Increment::from_step(0.01).unwrap();
        assert_eq!(Increment::unpack(cent.pack()), Some(cent));
        assert_eq!(Increment::unpack(0), None);
        assert_eq!(cent.round(150.256), 150.26);
        assert_eq!(cent.round(0.1 + 0.2), 0.3);
        let lot = Increment::from_step(0.0001).unwrap();
        assert_eq!(lot.round(0.40000000000000002), 0.4);
        assert_eq!(lot.round(12.34567), 12.3457);
        let five = Increment::from_step(0.05).unwrap();
        assert_eq!(five.round(1.12), 1.1);
        assert_eq!(five.round(1.13), 1.15);
        let hundred = Increment::from_step(100.0).unwrap();
        assert_eq!(hundred.round(260.0), 300.0);
        assert_eq!(cent.round(f64::MAX), f64::MAX);
        assert_eq!(Increment::from_step(0.0), None);
        assert_eq!(Increment::from_step(f64::MAX), None);
        assert_eq!(Increment::from_step(-1.0), None);
    }

    fn ev(id: InstrumentId, n: i64) -> MdEvent {
        MdEvent::new(id, MdStep::Quote).with(field::BID, n)
    }

    // A step reads back as written: header and values.
    #[test]
    fn a_step_reads_back_as_written() {
        let q = MdQueue::new();
        let mut e = MdEvent::new(255, MdStep::Last).with(field::LAST, -5).with(field::TIME, 1_790_000_000_000_000_000);
        e.halted = Some(2);
        e.auto = 0b1010;
        e.seen = 0b1111;
        assert!(q.push_now(&e));
        assert_eq!(q.read(q.tail()), e);
        let none = MdEvent::new(3, MdStep::CatchUp);
        assert!(q.push_now(&none));
        assert_eq!(q.read(1), none);
        // A message: its first step is marked.
        let mut second = MdEvent::new(3, MdStep::Quote);
        assert!(q.push_message(&[MdEvent::new(3, MdStep::Last), second]));
        second.first = false;
        assert_eq!((q.read(2).first, q.read(3)), (true, second));
    }

    // Only listened instruments are written; nothing is handed over before
    // `publish`.
    #[test]
    fn only_listened_instruments_are_written() {
        let q = MdQueue::new();
        assert!(!q.offer(&ev(4, 1)));
        q.listen(4, true);
        assert!(q.offer(&ev(4, 1)));
        assert_eq!(q.head(), 0);
        q.publish();
        assert_eq!(q.head(), 1);
        q.listen(4, false);
        assert!(!q.offer(&ev(4, 2)));
    }

    // A full ring drops the instrument's steps until a catch-up is written,
    // once the reader has made room; the catch-up comes before its later
    // steps. Other instruments go on as long as there is room.
    #[test]
    fn a_full_ring_gives_a_catch_up_once_there_is_room() {
        let q = MdQueue::new();
        q.listen(1, true);
        q.listen(2, true);
        for n in 0..MD_QUEUE_CAPACITY as i64 {
            assert!(q.offer(&ev(1, n)));
        }
        q.publish();
        assert!(!q.offer(&ev(1, -1)), "full");
        assert!(q.needs_catch_up(1) && q.catch_up_wanted());
        assert!(!q.offer(&ev(2, 7)), "full for every instrument");
        // Still full: no catch-up yet.
        q.write_catch_ups(|id| MdEvent::new(id, MdStep::CatchUp));
        assert!(q.needs_catch_up(1));
        // The reader takes all but a quarter: still over half full.
        q.release(MD_QUEUE_CAPACITY / 4);
        q.write_catch_ups(|id| MdEvent::new(id, MdStep::CatchUp));
        assert!(q.needs_catch_up(1));
        assert!(!q.offer(&ev(1, -2)), "waits for its catch-up");
        q.release(MD_QUEUE_CAPACITY);
        q.write_catch_ups(|id| MdEvent::new(id, MdStep::CatchUp).with(field::BID, 99));
        assert!(!q.needs_catch_up(1) && !q.needs_catch_up(2) && !q.catch_up_wanted());
        assert!(q.offer(&ev(1, 100)));
        q.publish();
        let steps: Vec<MdEvent> = (MD_QUEUE_CAPACITY..q.head()).map(|s| q.read(s)).collect();
        assert_eq!(steps.iter().map(|e| (e.instrument, e.step, e.values[field::BID])).collect::<Vec<_>>(), [
            (1, MdStep::CatchUp, 99), (2, MdStep::CatchUp, 99), (1, MdStep::Quote, 100),
        ]);
    }

    // A catch-up the reader asks for is written at the engine's next turn,
    // only for a listened instrument.
    #[test]
    fn a_catch_up_on_request() {
        let q = MdQueue::new();
        q.request_catch_up(9);
        q.write_catch_ups(|id| MdEvent::new(id, MdStep::CatchUp));
        assert_eq!(q.head(), 0, "not listened");
        q.listen(9, true);
        q.request_catch_up(9);
        q.write_catch_ups(|id| MdEvent::new(id, MdStep::CatchUp));
        assert_eq!(q.head(), 1);
        assert_eq!(q.read(0).step, MdStep::CatchUp);
    }

    // The default steps of a test message: the groups with values, in the
    // reference's order of a message with a trade, daily figures and a book.
    #[test]
    fn a_test_message_has_the_groups_with_values() {
        let q = Quote { bid: 1, ask: 2, last: 3, high: 4, timestamp_ns: 5, ..Default::default() };
        let evs = TestMessage::default().events(0, &q);
        let steps: Vec<(MdStep, u16)> = evs.iter().map(|e| (e.step, e.fields)).collect();
        assert_eq!(steps, [
            (MdStep::Time, 1 << field::TIME),
            (MdStep::Last, 1 << field::LAST),
            (MdStep::Daily, 1 << field::HIGH),
            (MdStep::Quote, 1 << field::BID | 1 << field::ASK),
        ]);
    }
}
