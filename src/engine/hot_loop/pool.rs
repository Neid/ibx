//! Data farms opened on demand (#445).
//!
//! The two primary farms of the logon (market data and historical) stay
//! where they were. Any other farm a routing row names is opened the first
//! time a request routes to it, as the reference does: the farm is created,
//! its messages wait in a queue (at most 1000) and go out after its logon.
//! An idle farm is closed: a market data farm by the activity check every
//! 120 s, a historical farm when nothing but heartbeats came for 360 s.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fixcomp;

/// A data farm: 0 and 1 are the primary market data and historical farms
/// of the logon; farms opened on demand follow.
pub(crate) type FarmId = u8;
pub(crate) const PRIMARY_MD: FarmId = 0;
pub(crate) const PRIMARY_HMDS: FarmId = 1;
const FIRST_ON_DEMAND: FarmId = 2;

/// Most messages kept for a farm that is not logged on yet; more are
/// dropped ("queue is full"), as in the reference.
pub(crate) const QUEUE_LIMIT: usize = 1000;

/// Period of the market data activity check, and how long the last request
/// to a farm must be past before it is closed.
pub(crate) const MD_ACTIVITY_PERIOD: Duration = Duration::from_secs(120);
/// A farm that was sent anything this recently is not closed.
pub(crate) const MD_RECENT_SEND: Duration = Duration::from_secs(5);
/// Period of the historical farm ping, which also runs the dormant test.
pub(crate) const HMDS_PING_PERIOD: Duration = Duration::from_secs(60);
/// A historical farm with nothing but heartbeats for longer is dormant.
pub(crate) const HMDS_DORMANT_AFTER: Duration = Duration::from_secs(360);
/// The period of the check of a market data farm that is connecting: the
/// "farm is connecting" notice is given again when subscriptions were
/// queued for it since the previous check (ibx#684; the reference 10.51,
/// observed in the recordings of 13-16/10/2026: 82 of 82 notices of 42
/// connections of the option farm).
pub(crate) const CONNECTING_CHECK: Duration = Duration::from_millis(500);

/// The request type of a market data message, and its value for a
/// cancel: a queued message of any other type is a subscription.
const REQUEST_TYPE: u32 = 263;
const REQUEST_CANCEL: &str = "2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FarmKind {
    MarketData,
    Historical,
    /// The contract data farm, for the calendar data requests (ibx#443).
    /// Its state is reported with the auth link's, as at the connect.
    SecDef,
}

/// Where a request's messages go: a live connection, or the queue of a
/// farm that is still logging on.
pub(crate) trait FixSink {
    /// A plain message (an XML request of the historical farm); false
    /// when it was neither sent nor queued.
    fn send_plain(&mut self, fields: &[(u32, &str)]) -> bool;
    /// A compressed message; false when it was neither sent nor queued.
    fn send_comp(&mut self, fields: &[(u32, &str)]) -> bool;
}

/// Send a plain message as the reference writes its requests to the
/// historical farm: the XML in its writer's layout and the sequence
/// number 000000 (every query, cancel and gateway-specific message of the
/// four-leg recordings of 26/09 to 02/10/2026, ibx#486; the heartbeats keep
/// their count).
pub(crate) fn send_plain_on(conn: &mut Connection, fields: &[(u32, &str)]) -> io::Result<()> {
    let laid_out: Vec<(u32, String)> = fields.iter().map(|(t, v)| {
        (*t, if *t == 6118 { crate::protocol::fix::xml_layout(v) } else { v.to_string() })
    }).collect();
    conn.send_fix_unsequenced(&borrowed(&laid_out))
}

impl FixSink for Option<Connection> {
    fn send_plain(&mut self, fields: &[(u32, &str)]) -> bool {
        self.as_mut().is_some_and(|c| send_plain_on(c, fields).is_ok())
    }
    fn send_comp(&mut self, fields: &[(u32, &str)]) -> bool {
        self.as_mut().is_some_and(|c| c.send_fixcomp(fields).is_ok())
    }
}

#[derive(Debug)]
enum Queued {
    Plain(Vec<(u32, String)>),
    Comp(Vec<(u32, String)>),
}

fn owned(fields: &[(u32, &str)]) -> Vec<(u32, String)> {
    fields.iter().map(|(t, v)| (*t, v.to_string())).collect()
}

/// A farm opened on demand.
pub(crate) struct OnDemandFarm {
    pub(crate) id: FarmId,
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) kind: FarmKind,
    pub(crate) conn: Option<Connection>,
    connecting: Option<Receiver<io::Result<Connection>>>,
    /// Set by the connect under way once its socket is connected.
    socket_up: Option<Arc<AtomicBool>>,
    /// A connect began and the farm is not logged on yet.
    connecting_phase: bool,
    /// A client's market data request waits for a connection not started
    /// yet: its start gives the connecting notice at once.
    client_start: bool,
    /// The connecting notice of a connect start, given at the next poll.
    notice_now: bool,
    /// The next connecting check, from the first subscription queued.
    next_check: Option<Instant>,
    /// A check waited for the socket connect.
    check_held: bool,
    /// Subscription messages queued since the previous check.
    new_subs: u32,
    queue: VecDeque<Queued>,
    /// A connection is wanted: messages wait, or requests still use it.
    pub(crate) wanted: bool,
    retry_at: Option<Instant>,
    attempts: u32,
    /// Last market data request sent (activity check).
    pub(crate) last_request: Instant,
    /// Last message of any kind sent.
    pub(crate) last_send: Instant,
    /// Last message received that is not a heartbeat (dormant test).
    pub(crate) last_activity: Instant,
    /// Next tick of the historical farm ping.
    next_ping: Instant,
    /// Next periodic test request, and the one waiting for an answer
    /// (the primary farms' rule, `liveness`).
    next_test: Instant,
    pending_test: Option<Instant>,
}

impl OnDemandFarm {
    fn new(id: FarmId, name: &str, host: &str, kind: FarmKind, now: Instant) -> Self {
        Self {
            id,
            name: name.to_string(),
            host: host.to_string(),
            kind,
            conn: None,
            connecting: None,
            socket_up: None,
            connecting_phase: false,
            client_start: false,
            notice_now: false,
            next_check: None,
            check_held: false,
            new_subs: 0,
            queue: VecDeque::new(),
            wanted: false,
            retry_at: None,
            attempts: 0,
            last_request: now,
            last_send: now,
            last_activity: now,
            next_ping: now + HMDS_PING_PERIOD,
            next_test: now + super::liveness::FARM_PING_INTERVAL,
            pending_test: None,
        }
    }

    fn enqueue(&mut self, msg: Queued) -> bool {
        self.wanted = true;
        if self.queue.len() >= QUEUE_LIMIT {
            log::warn!("{}: not added message (queue is full)", self.name);
            return false;
        }
        log::debug!("{}: added message to queue ({} waiting)", self.name, self.queue.len() + 1);
        self.queue.push_back(msg);
        true
    }

    /// Messages waiting for the logon.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Logged on and usable.
    #[cfg(test)]
    pub(crate) fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    /// Record a market data request sent or queued (activity check).
    pub(crate) fn note_request(&mut self, now: Instant) {
        self.last_request = now;
    }

    /// A client's market data request routed to this farm: when it starts
    /// the farm's connection, the connecting notice goes at once (ibx#684;
    /// the reference 10.51, observed in the recordings of 13-16/10/2026;
    /// a connection started by the engine itself, as an option model's,
    /// gives none at its start).
    pub(crate) fn note_client_request(&mut self) {
        if self.conn.is_none() && !self.connecting_phase {
            self.client_start = true;
        }
    }

    /// A subscription queued for the farm while it is not logged on: the
    /// checks run every 0.5 s from the first (ibx#684).
    fn note_queued_subscription(&mut self, now: Instant) {
        if self.kind != FarmKind::MarketData {
            return;
        }
        if self.next_check.is_none() {
            self.next_check = Some(now + CONNECTING_CHECK);
        }
        self.new_subs += 1;
    }

    /// The connecting notice is due: at the start of a connect a client's
    /// request began, then at each check that finds new subscriptions
    /// queued; a check waits while the socket connect runs, and runs as
    /// soon as it ended (ibx#684). The notice of the start stands for one
    /// subscription message queued: a request with one message gives one
    /// notice however long the connect takes (ibx#666; the reference 10.51,
    /// observed in the recordings of 13-16/10/2026: 14 of 14 connects a
    /// client's request or a fund order began with one message queued gave
    /// one notice, logged on 0.68 to 2.28 s later; 2 of 2 with two messages
    /// queued, a stock's pair and its generic tick, gave a second at the
    /// check of 0.5 s).
    fn connecting_notice(&mut self, now: Instant) -> bool {
        if self.kind != FarmKind::MarketData || !self.connecting_phase || self.conn.is_some() {
            return false;
        }
        if std::mem::take(&mut self.notice_now) {
            self.new_subs = self.new_subs.saturating_sub(1);
            return true;
        }
        let Some(at) = self.next_check else { return false };
        if now < at {
            return false;
        }
        if self.connecting.is_some() && self.socket_up.as_ref().is_some_and(|s| !s.load(Ordering::Acquire)) {
            self.check_held = true;
            return false;
        }
        let ran = if std::mem::take(&mut self.check_held) { now } else { at };
        self.next_check = Some(ran + CONNECTING_CHECK);
        std::mem::take(&mut self.new_subs) > 0
    }

    /// The farm logged on: the connecting phase ends.
    fn end_connecting_phase(&mut self) {
        self.connecting_phase = false;
        self.client_start = false;
        self.notice_now = false;
        self.next_check = None;
        self.check_held = false;
        self.new_subs = 0;
        self.socket_up = None;
    }
}

/// A market data message that is not a cancel: a subscription.
fn is_subscription(fields: &[(u32, &str)]) -> bool {
    fields.iter().any(|(t, v)| *t == REQUEST_TYPE && *v != REQUEST_CANCEL)
}

impl FixSink for OnDemandFarm {
    fn send_plain(&mut self, fields: &[(u32, &str)]) -> bool {
        self.last_send = Instant::now();
        match self.conn.as_mut() {
            Some(c) => send_plain_on(c, fields).is_ok(),
            None => self.enqueue(Queued::Plain(owned(fields))),
        }
    }
    fn send_comp(&mut self, fields: &[(u32, &str)]) -> bool {
        let now = Instant::now();
        self.last_send = now;
        match self.conn.as_mut() {
            Some(c) => c.send_fixcomp(fields).is_ok(),
            None => {
                let queued = self.enqueue(Queued::Comp(owned(fields)));
                if queued && is_subscription(fields) {
                    self.note_queued_subscription(now);
                }
                queued
            }
        }
    }
}

/// What happened to a farm, for the client notices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FarmEvent {
    /// Still connecting some time after the connect began.
    Connecting(FarmId),
    /// Logged on; the queue was sent.
    Connected(FarmId),
    /// The connection was lost.
    Lost(FarmId),
    /// Closed for inactivity; opened again on the next request.
    IdleClosed(FarmId),
}

/// The client notice of a farm event: code and text, as the reference
/// words them (the farm name follows a `:`, or directly a text that ends
/// with a `.`).
pub(crate) fn farm_notice(kind: FarmKind, event: &FarmEvent, name: &str) -> (i64, String) {
    let (code, text) = match (kind, event) {
        (FarmKind::MarketData, FarmEvent::Connecting(_)) => (2119, "Market data farm is connecting"),
        (FarmKind::MarketData, FarmEvent::Connected(_)) => (2104, "Market data farm connection is OK"),
        (FarmKind::MarketData, FarmEvent::Lost(_)) => (2103, "Market data farm connection is broken"),
        (FarmKind::MarketData, FarmEvent::IdleClosed(_)) =>
            (2108, "Market data farm connection is inactive but should be available upon demand."),
        (FarmKind::Historical, FarmEvent::Connecting(_)) => (2120, "HMDS data farm is connecting"),
        (FarmKind::Historical, FarmEvent::Connected(_)) => (2106, "HMDS data farm connection is OK"),
        (FarmKind::Historical, FarmEvent::Lost(_)) => (2105, "HMDS data farm connection is broken"),
        (FarmKind::Historical, FarmEvent::IdleClosed(_)) =>
            (2107, "HMDS data farm connection is inactive but should be available upon demand."),
        (FarmKind::SecDef, FarmEvent::Connecting(_) | FarmEvent::Connected(_)) => (2158, "Sec-def data farm connection is OK"),
        (FarmKind::SecDef, FarmEvent::Lost(_) | FarmEvent::IdleClosed(_)) => (2157, "Sec-def data farm connection is broken"),
    };
    let sep = if text.ends_with([':', '.', '=', '-']) { "" } else { ":" };
    (code, format!("{}{}{}", text, sep, name))
}

/// Read the messages a farm connection has: compressed frames inflated,
/// signatures checked. Err when the connection is lost; the flag is set
/// when a signature did not match (the connection must be dropped).
pub(crate) fn read_messages(conn: &mut Connection) -> io::Result<(Vec<Vec<u8>>, bool)> {
    match conn.try_recv() {
        Ok(0) if !conn.has_buffered_data() => return Ok((Vec::new(), false)),
        Ok(_) => {}
        Err(e) => return Err(e),
    }
    let mut msgs = Vec::new();
    for frame in conn.extract_frames() {
        match frame {
            Frame::FixComp(raw) => {
                let (unsigned, valid) = conn.unsign(&raw);
                if !valid {
                    return Ok((msgs, true));
                }
                match fixcomp::fixcomp_decompress(&unsigned) {
                    Ok(inner) => msgs.extend(inner),
                    Err(e) => log::warn!("Farm: dropping malformed compressed frame ({} bytes): {}", unsigned.len(), e),
                }
            }
            Frame::Binary(raw) | Frame::Fix(raw) => {
                let (unsigned, valid) = conn.unsign(&raw);
                if !valid {
                    return Ok((msgs, true));
                }
                msgs.push(unsigned);
            }
            Frame::Control(_) => {}
        }
    }
    Ok((msgs, false))
}

/// The farms opened on demand.
pub(crate) struct FarmPool {
    pub(crate) farms: Vec<OnDemandFarm>,
    /// Next run of the market data activity check.
    next_md_check: Instant,
}

impl FarmPool {
    pub(crate) fn new(now: Instant) -> Self {
        Self { farms: Vec::new(), next_md_check: now + MD_ACTIVITY_PERIOD }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.farms.is_empty()
    }

    /// The farm of that name, if it was opened before.
    pub(crate) fn find(&self, name: &str) -> Option<FarmId> {
        self.farms.iter().find(|f| f.name == name).map(|f| f.id)
    }

    /// The farm of that name, created when it is new. Its connect starts
    /// when a message waits for it.
    pub(crate) fn ensure(&mut self, name: &str, host: &str, kind: FarmKind, now: Instant) -> FarmId {
        if let Some(id) = self.find(name) {
            let farm = self.get_mut(id).expect("found farm");
            if !host.is_empty() {
                farm.host = host.to_string();
            }
            return id;
        }
        let id = FIRST_ON_DEMAND + self.farms.len() as FarmId;
        log::info!("Farm {} ({}) created on demand", name, host);
        self.farms.push(OnDemandFarm::new(id, name, host, kind, now));
        id
    }

    pub(crate) fn get(&self, id: FarmId) -> Option<&OnDemandFarm> {
        id.checked_sub(FIRST_ON_DEMAND).and_then(|i| self.farms.get(i as usize))
    }

    pub(crate) fn get_mut(&mut self, id: FarmId) -> Option<&mut OnDemandFarm> {
        id.checked_sub(FIRST_ON_DEMAND).and_then(|i| self.farms.get_mut(i as usize))
    }

    /// Farms whose connect should start now: wanted, not connected, no
    /// connect under way, retry time reached.
    pub(crate) fn due_connects(&self, now: Instant) -> Vec<FarmId> {
        self.farms.iter()
            .filter(|f| f.wanted && f.conn.is_none() && f.connecting.is_none())
            .filter(|f| f.retry_at.is_none_or(|t| now >= t))
            .map(|f| f.id)
            .collect()
    }

    /// A connect began; its result comes on `rx`, and `socket_up` is set
    /// once its socket is connected. The first of a connecting phase that
    /// a client's request began gives the connecting notice at once.
    pub(crate) fn set_connecting(&mut self, id: FarmId, rx: Receiver<io::Result<Connection>>, socket_up: Arc<AtomicBool>) {
        if let Some(f) = self.get_mut(id) {
            f.connecting = Some(rx);
            f.socket_up = Some(socket_up);
            f.attempts += 1;
            if !f.connecting_phase {
                f.connecting_phase = true;
                f.notice_now = std::mem::take(&mut f.client_start);
            }
        }
    }

    /// Finished connects, and the connecting notices due.
    pub(crate) fn poll_connects(&mut self, now: Instant) -> (Vec<(FarmId, io::Result<Connection>)>, Vec<FarmEvent>) {
        let mut done = Vec::new();
        let mut events = Vec::new();
        for f in &mut self.farms {
            if let Some(rx) = f.connecting.as_ref() {
                match rx.try_recv() {
                    Ok(result) => {
                        f.connecting = None;
                        done.push((f.id, result));
                        continue;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        f.connecting = None;
                        done.push((f.id, Err(io::Error::other("connect thread ended without a result"))));
                        continue;
                    }
                }
            }
            if f.connecting_notice(now) {
                events.push(FarmEvent::Connecting(f.id));
            }
        }
        (done, events)
    }

    /// The farm logged on: the waiting messages go out in order.
    pub(crate) fn on_connected(&mut self, id: FarmId, mut conn: Connection, now: Instant) -> FarmEvent {
        // Each link writes on its own, as every engine link (ibx#254).
        conn.set_queued_writes(true);
        if let Some(f) = self.get_mut(id) {
            f.conn = Some(conn);
            f.end_connecting_phase();
            f.wanted = false;
            f.retry_at = None;
            f.attempts = 0;
            f.last_activity = now;
            f.next_ping = now + HMDS_PING_PERIOD;
            f.next_test = now + super::liveness::FARM_PING_INTERVAL;
            f.pending_test = None;
            let waiting: Vec<Queued> = f.queue.drain(..).collect();
            if !waiting.is_empty() {
                log::info!("{}: sending saved messages ({})", f.name, waiting.len());
            }
            let conn = f.conn.as_mut().expect("just set");
            for msg in waiting {
                let sent = match &msg {
                    Queued::Plain(fields) => send_plain_on(conn, &borrowed(fields)),
                    Queued::Comp(fields) => conn.send_fixcomp(&borrowed(fields)),
                };
                if let Err(e) = sent {
                    log::warn!("{}: saved message not sent: {}", f.name, e);
                }
            }
            f.last_send = now;
        }
        FarmEvent::Connected(id)
    }

    /// A connect failed: tried again later while the farm is wanted.
    pub(crate) fn on_connect_failed(&mut self, id: FarmId, now: Instant) {
        if let Some(f) = self.get_mut(id) {
            let delay = super::hmds_reconnect_backoff(f.attempts.max(1));
            log::warn!("{}: connect failed (attempt {}), next try in {:?}", f.name, f.attempts, delay);
            f.retry_at = Some(now + delay);
        }
    }

    /// The connection was lost: dropped; connected again when a request
    /// still uses it (`wanted`).
    pub(crate) fn on_lost(&mut self, id: FarmId, wanted: bool, now: Instant) -> FarmEvent {
        if let Some(f) = self.get_mut(id) {
            if let Some(c) = f.conn.as_mut() {
                c.shutdown();
            }
            f.conn = None;
            f.wanted = wanted || !f.queue.is_empty();
            f.retry_at = Some(now + Duration::from_secs(1));
        }
        FarmEvent::Lost(id)
    }

    /// Close a farm for inactivity: nothing is kept; the next request
    /// opens it again.
    pub(crate) fn close_idle(&mut self, id: FarmId) -> FarmEvent {
        if let Some(f) = self.get_mut(id) {
            log::info!("Disconnecting {} when checking activity", f.name);
            if let Some(c) = f.conn.as_mut() {
                c.shutdown();
            }
            f.conn = None;
            f.wanted = false;
            f.retry_at = None;
        }
        FarmEvent::IdleClosed(id)
    }

    /// The market data activity check, every 120 s: a farm opened on
    /// demand is closed when no live request uses it (`in_use`), its last
    /// request is 120 s old, it was sent nothing in the last 5 s, and
    /// another market data farm stays connected (`others_up` counts the
    /// connected ones outside the pool). The primary farms are not in the
    /// pool, so they are never closed.
    pub(crate) fn md_activity_check(&mut self, now: Instant, others_up: usize, in_use: impl Fn(FarmId) -> bool) -> Vec<FarmId> {
        if now < self.next_md_check {
            return Vec::new();
        }
        self.next_md_check = now + MD_ACTIVITY_PERIOD;
        let mut up = others_up + self.farms.iter().filter(|f| f.kind == FarmKind::MarketData && f.conn.is_some()).count();
        let mut close = Vec::new();
        for f in &self.farms {
            if f.kind != FarmKind::MarketData || f.conn.is_none() || !f.queue.is_empty() {
                continue;
            }
            let idle = !in_use(f.id)
                && now.duration_since(f.last_request) >= MD_ACTIVITY_PERIOD
                && now.duration_since(f.last_send) >= MD_RECENT_SEND;
            if idle && up > 1 {
                up -= 1;
                close.push(f.id);
            }
        }
        close
    }

    /// The historical farm ping tick, every 60 s after the logon: a farm is
    /// dormant when nothing but heartbeats came for more than 360 s and no
    /// historical farm has a live listener (streaming bars, tick-by-tick,
    /// scanner).
    pub(crate) fn hmds_dormant_check(&mut self, now: Instant, any_listener: bool) -> Vec<FarmId> {
        let mut close = Vec::new();
        for f in &mut self.farms {
            if f.kind != FarmKind::Historical || f.conn.is_none() || now < f.next_ping {
                continue;
            }
            f.next_ping += HMDS_PING_PERIOD;
            if !any_listener && f.queue.is_empty() && now.duration_since(f.last_activity) > HMDS_DORMANT_AFTER {
                log::info!("Closing dormant connection type:HISTORICAL_DATA {}", f.name);
                close.push(f.id);
            }
        }
        close
    }

    /// Liveness of the farms opened on demand, with the primary farms'
    /// rule (ibx#419): a test request every 60 s from the logon, and the
    /// link is dead when nothing arrives within 10 s of it. `next_id`
    /// numbers the test requests with the engine's counter. Returns the
    /// farms found dead.
    pub(crate) fn liveness(&mut self, now: Instant, next_id: &mut dyn FnMut() -> String) -> Vec<FarmId> {
        let mut dead = Vec::new();
        for f in &mut self.farms {
            let Some(conn) = f.conn.as_mut() else { continue };
            if f.pending_test.is_some_and(|sent| now.saturating_duration_since(sent) >= super::liveness::FARM_PING_ANSWER) {
                log::error!("{}: no answer to the test request, connection reset", f.name);
                dead.push(f.id);
                continue;
            }
            if now >= f.next_test {
                f.next_test += super::liveness::FARM_PING_INTERVAL;
                if f.next_test <= now {
                    f.next_test = now + super::liveness::FARM_PING_INTERVAL;
                }
                let _ = super::send_farm_ping(conn, &next_id());
                f.pending_test = Some(now);
            }
        }
        dead
    }

    /// Send what each farm's socket did not take yet (ibx#254). Returns the
    /// farms whose write failed: their link is dropped.
    pub(crate) fn flush_writes(&mut self) -> Vec<FarmId> {
        let mut failed = Vec::new();
        for f in &mut self.farms {
            let Some(conn) = f.conn.as_mut() else { continue };
            if conn.has_queued_output() {
                let _ = conn.flush_queued();
            }
            if let Some(error) = conn.write_error() {
                log::error!("{}: send failed: {}", f.name, error);
                failed.push(f.id);
            }
        }
        failed
    }

    /// Something came on the farm: `activity` when it is not a heartbeat.
    pub(crate) fn note_received(&mut self, id: FarmId, activity: bool, now: Instant) {
        if let Some(f) = self.get_mut(id) {
            f.pending_test = None;
            if activity {
                f.last_activity = now;
            }
        }
    }
}

fn borrowed(fields: &[(u32, String)]) -> Vec<(u32, &str)> {
    fields.iter().map(|(t, v)| (*t, v.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> (Connection, crate::protocol::connection::MemTransport) {
        let (client, server) = crate::protocol::connection::mem_pair();
        (Connection::new_mem(client), server)
    }

    fn read_all(server: &mut crate::protocol::connection::MemTransport) -> Vec<u8> {
        use std::io::Read;
        server.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = server.read(&mut buf) {
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    // Messages for a farm not logged on wait in its queue, at most 1000,
    // and go out in order after the logon.
    #[test]
    fn queue_until_logon_then_flush_in_order() {
        let now = Instant::now();
        let mut pool = FarmPool::new(now);
        let id = pool.ensure("cashfarm", "ndc1.example", FarmKind::MarketData, now);
        assert_eq!(id, 2);
        assert_eq!(pool.ensure("cashfarm", "", FarmKind::MarketData, now), id, "one farm per name");
        let farm = pool.get_mut(id).unwrap();
        assert!(farm.send_comp(&[(35, "V"), (262, "1")]));
        assert!(farm.send_plain(&[(35, "W"), (6118, "<x/>")]));
        assert_eq!(farm.queued(), 2);
        assert_eq!(pool.due_connects(now), vec![id], "a waiting message opens the farm");
        let farm = pool.get_mut(id).unwrap();
        for _ in 2..QUEUE_LIMIT {
            assert!(farm.send_comp(&[(35, "V")]));
        }
        assert!(!farm.send_comp(&[(35, "V")]), "queue full: dropped");
        assert_eq!(farm.queued(), QUEUE_LIMIT);

        let (conn, mut server) = loopback();
        assert_eq!(pool.on_connected(id, conn, now), FarmEvent::Connected(id));
        assert_eq!(pool.get(id).unwrap().queued(), 0);
        assert!(pool.due_connects(now).is_empty());
        let wire = read_all(&mut server);
        assert!(wire.starts_with(b"8=FIXCOMP"), "the first saved message goes first");
        assert!(wire.windows(9).any(|w| w == b"6118=<x/>"));
    }

    // A failed connect is tried again after a backoff while wanted.
    #[test]
    fn failed_connect_is_retried_later() {
        let now = Instant::now();
        let mut pool = FarmPool::new(now);
        let id = pool.ensure("fundfarm", "h", FarmKind::Historical, now);
        pool.get_mut(id).unwrap().send_plain(&[(35, "U")]);
        let (_tx, rx) = crossbeam_channel::bounded(1);
        pool.set_connecting(id, rx, Arc::new(AtomicBool::new(false)));
        assert!(pool.due_connects(now).is_empty(), "connect under way");
        pool.get_mut(id).unwrap().connecting = None;
        pool.on_connect_failed(id, now);
        assert!(pool.due_connects(now).is_empty());
        assert_eq!(pool.due_connects(now + Duration::from_secs(4)), vec![id]);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A market data subscription, and its cancel.
    const SUB: &[(u32, &str)] = &[(35, "V"), (263, "1"), (262, "7")];
    const CANCEL: &[(u32, &str)] = &[(35, "V"), (263, "2"), (262, "7")];

    /// The connecting notices of a poll.
    fn notices(pool: &mut FarmPool, at: Instant) -> usize {
        pool.poll_connects(at).1.iter().filter(|e| matches!(e, FarmEvent::Connecting(_))).count()
    }

    /// A market data farm with a first subscription queued at `t0`, its
    /// connect started (by a client's request when `client`); the socket
    /// connect's flag.
    fn connecting_farm(t0: Instant, client: bool) -> (FarmPool, FarmId, Arc<AtomicBool>, crossbeam_channel::Sender<io::Result<Connection>>) {
        let mut pool = FarmPool::new(t0);
        let id = pool.ensure("usopt", "h", FarmKind::MarketData, t0);
        let f = pool.get_mut(id).unwrap();
        if client {
            f.note_client_request();
        }
        assert!(f.send_comp(SUB));
        // The first subscription queued at t0.
        f.next_check = Some(t0 + CONNECTING_CHECK);
        assert_eq!(pool.due_connects(t0), vec![id]);
        let (tx, rx) = crossbeam_channel::bounded(1);
        let socket = Arc::new(AtomicBool::new(true));
        pool.set_connecting(id, rx, socket.clone());
        (pool, id, socket, tx)
    }

    // ibx#684, the reference 10.51 (observed in the recordings of
    // 13-16/10/2026): a client's request that starts the farm's connection
    // gives the notice at once; the check every 0.5 s from the first
    // subscription gives it again when subscriptions were queued since the
    // previous check, not for a cancel, and not without new ones.
    #[test]
    fn a_client_request_gives_the_connecting_notice_at_once_then_on_new_subscriptions() {
        let t0 = Instant::now();
        let (mut pool, id, _socket, tx) = connecting_farm(t0, true);
        assert_eq!(notices(&mut pool, t0), 1, "at once");
        assert_eq!(notices(&mut pool, t0 + ms(200)), 0);
        // A second request queued 0.3 s later: the 0.5 s check.
        assert!(pool.get_mut(id).unwrap().send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(499)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(500)), 1, "new subscriptions since the start");
        // Nothing new: no repeat.
        assert_eq!(notices(&mut pool, t0 + ms(1000)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(1500)), 0);
        // A cancel does not count.
        assert!(pool.get_mut(id).unwrap().send_comp(CANCEL));
        assert_eq!(notices(&mut pool, t0 + ms(2000)), 0);
        // A subscription queued at 2.2 s: the check of 2.5 s.
        assert!(pool.get_mut(id).unwrap().send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(2400)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(2501)), 1);
        // Logged on: no more checks.
        let (conn, _server) = loopback();
        tx.send(Ok(conn)).unwrap();
        let (done, events) = pool.poll_connects(t0 + ms(2700));
        assert!(events.is_empty());
        for (id, result) in done {
            pool.on_connected(id, result.unwrap(), t0 + ms(2700));
        }
        assert!(pool.get_mut(id).unwrap().send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(3000)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(3600)), 0);
    }

    // ibx#666, the reference 10.51 (observed in the recordings of
    // 13-16/10/2026): the notice at the start stands for one message
    // queued. A client's request of one message gives one notice however
    // long the connect takes; of two messages (a pair and a generic tick),
    // a second notice at the check.
    #[test]
    fn the_start_notice_stands_for_one_queued_subscription() {
        let t0 = Instant::now();
        let (mut pool, _id, _socket, _tx) = connecting_farm(t0, true);
        assert_eq!(notices(&mut pool, t0), 1, "at once");
        assert_eq!(notices(&mut pool, t0 + ms(500)), 0, "the same subscription");
        assert_eq!(notices(&mut pool, t0 + ms(1000)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(2500)), 0);

        let mut pool = FarmPool::new(t0);
        let id = pool.ensure("eufarm", "h", FarmKind::MarketData, t0);
        let f = pool.get_mut(id).unwrap();
        f.note_client_request();
        assert!(f.send_comp(SUB));
        assert!(f.send_comp(SUB));
        f.next_check = Some(t0 + CONNECTING_CHECK);
        let (_tx, rx) = crossbeam_channel::bounded(1);
        pool.set_connecting(id, rx, Arc::new(AtomicBool::new(true)));
        assert_eq!(notices(&mut pool, t0), 1, "at once");
        assert_eq!(notices(&mut pool, t0 + ms(500)), 1, "the second message");
        assert_eq!(notices(&mut pool, t0 + ms(1000)), 0);
    }

    // A connection the engine started (an option model's): no notice at
    // its start; the check gives it for the subscriptions queued, its own
    // first, then a client's queued later. A connect tried again after a
    // failure gives none at its start.
    #[test]
    fn an_engine_started_connect_gives_no_notice_at_its_start() {
        let t0 = Instant::now();
        let (mut pool, id, _socket, tx) = connecting_farm(t0, false);
        assert_eq!(notices(&mut pool, t0), 0, "not at its start");
        assert_eq!(notices(&mut pool, t0 + ms(500)), 1, "the check: its own subscription");
        assert_eq!(notices(&mut pool, t0 + ms(1000)), 0);
        // A client's request now: queued, its notice at the next check.
        let f = pool.get_mut(id).unwrap();
        f.note_client_request();
        assert!(f.send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(1200)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(1500)), 1);
        // The connect fails and is tried again: no notice at the new start.
        drop(tx);
        let (done, _) = pool.poll_connects(t0 + ms(1600));
        assert_eq!(done.len(), 1);
        pool.on_connect_failed(id, t0 + ms(1600));
        let (_tx, rx) = crossbeam_channel::bounded(1);
        pool.set_connecting(id, rx, Arc::new(AtomicBool::new(true)));
        assert_eq!(notices(&mut pool, t0 + ms(4000)), 0);
    }

    // A check waits while the socket connect runs and comes as soon as
    // it ended; the next one 0.5 s after it.
    #[test]
    fn a_connecting_check_waits_for_the_socket_connect() {
        let t0 = Instant::now();
        let (mut pool, id, socket, _tx) = connecting_farm(t0, true);
        socket.store(false, Ordering::Release);
        assert_eq!(notices(&mut pool, t0), 1, "the start's notice does not wait");
        assert!(pool.get_mut(id).unwrap().send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(500)), 0, "held");
        assert_eq!(notices(&mut pool, t0 + ms(1200)), 0, "held");
        socket.store(true, Ordering::Release);
        assert_eq!(notices(&mut pool, t0 + ms(1207)), 1, "right after the socket connect");
        assert!(pool.get_mut(id).unwrap().send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(1700)), 0);
        assert_eq!(notices(&mut pool, t0 + ms(1707)), 1);
    }

    // No connecting notice for a historical farm, nor for a farm whose
    // connect never started.
    #[test]
    fn no_connecting_notice_without_a_connect_of_a_market_data_farm() {
        let t0 = Instant::now();
        let mut pool = FarmPool::new(t0);
        let md = pool.ensure("usopt", "h", FarmKind::MarketData, t0);
        let f = pool.get_mut(md).unwrap();
        f.note_client_request();
        assert!(f.send_comp(SUB));
        assert_eq!(notices(&mut pool, t0 + ms(600)), 0, "no connect began");
        let hist = pool.ensure("ushmds", "h", FarmKind::Historical, t0);
        assert!(pool.get_mut(hist).unwrap().send_comp(SUB));
        let (_tx, rx) = crossbeam_channel::bounded(1);
        pool.set_connecting(hist, rx, Arc::new(AtomicBool::new(true)));
        assert_eq!(notices(&mut pool, t0 + ms(2000)), 0);
    }

    // The activity check runs every 120 s and closes a market data farm
    // with no live request, whose last request is 120 s old, while another
    // farm stays up.
    #[test]
    fn md_farm_closed_by_the_activity_check() {
        let t0 = Instant::now();
        let mut pool = FarmPool::new(t0);
        let id = pool.ensure("usfuture", "h", FarmKind::MarketData, t0);
        let (conn, _server) = loopback();
        pool.on_connected(id, conn, t0);
        pool.get_mut(id).unwrap().note_request(t0);

        // Before the first period: no check.
        assert!(pool.md_activity_check(t0 + Duration::from_secs(119), 1, |_| false).is_empty());
        // A live request keeps it.
        assert!(pool.md_activity_check(t0 + Duration::from_secs(120), 1, |_| true).is_empty());
        // Next period: not used, last request 240 s old, primary up.
        assert!(pool.md_activity_check(t0 + Duration::from_secs(200), 1, |_| false).is_empty(), "next check at 240 s");
        assert_eq!(pool.md_activity_check(t0 + Duration::from_secs(240), 1, |_| false), vec![id]);

        // The last market data farm up is never closed.
        let t1 = t0 + Duration::from_secs(360);
        assert!(pool.md_activity_check(t1, 0, |_| false).is_empty());
        // A request 100 s before the check keeps it one more period.
        pool.get_mut(id).unwrap().note_request(t1 + Duration::from_secs(20));
        assert!(pool.md_activity_check(t1 + Duration::from_secs(120), 1, |_| false).is_empty());
        assert_eq!(pool.md_activity_check(t1 + Duration::from_secs(240), 1, |_| false), vec![id]);
        assert_eq!(pool.close_idle(id), FarmEvent::IdleClosed(id));
        assert!(!pool.get(id).unwrap().is_connected());
        assert!(pool.due_connects(t1 + Duration::from_secs(300)).is_empty(), "not opened again until a request");
    }

    // A historical farm is closed at a 60 s ping tick once nothing but
    // heartbeats came for more than 360 s, unless a live listener exists.
    #[test]
    fn hmds_farm_closed_when_dormant() {
        let t0 = Instant::now();
        let mut pool = FarmPool::new(t0);
        let id = pool.ensure("cashhmds", "h", FarmKind::Historical, t0);
        let (conn, _server) = loopback();
        pool.on_connected(id, conn, t0);
        pool.note_received(id, true, t0 + Duration::from_secs(10));
        for tick in 1..=6 {
            assert!(pool.hmds_dormant_check(t0 + Duration::from_secs(60 * tick), false).is_empty(), "tick {}", tick);
        }
        // Heartbeats only do not count.
        pool.note_received(id, false, t0 + Duration::from_secs(400));
        assert_eq!(pool.hmds_dormant_check(t0 + Duration::from_secs(420), false), vec![id]);

        let id2 = pool.ensure("euhmds", "h", FarmKind::Historical, t0);
        let (conn, _server2) = loopback();
        pool.on_connected(id2, conn, t0);
        assert!(pool.hmds_dormant_check(t0 + Duration::from_secs(420), true).is_empty(), "a live listener keeps it");
    }

    // ibx#419 rule on a farm opened on demand: a test request every 60 s
    // from the logon; an answer clears it; none within 10 s is a dead link.
    #[test]
    fn farm_test_request_every_minute_dead_without_answer() {
        let t0 = Instant::now();
        let mut pool = FarmPool::new(t0);
        let id = pool.ensure("usfuture", "h", FarmKind::MarketData, t0);
        let (conn, mut server) = loopback();
        pool.on_connected(id, conn, t0);
        let mut n = 0;
        let mut next = || { n += 1; format!("FixTestRequest{n}") };
        assert!(pool.liveness(t0 + Duration::from_secs(59), &mut next).is_empty());
        assert!(pool.liveness(t0 + Duration::from_secs(60), &mut next).is_empty());
        let _ = pool.flush_writes();
        let wire = read_all(&mut server);
        assert!(wire.windows(19).any(|w| w == b"112=FixTestRequest1"), "{}", String::from_utf8_lossy(&wire));
        pool.note_received(id, false, t0 + Duration::from_secs(61));
        assert!(pool.liveness(t0 + Duration::from_secs(75), &mut next).is_empty(), "answered");
        assert!(pool.liveness(t0 + Duration::from_secs(120), &mut next).is_empty());
        assert_eq!(pool.liveness(t0 + Duration::from_secs(130), &mut next), vec![id]);
    }

    #[test]
    fn notices_name_the_farm_as_the_reference() {
        let n = |k, e: FarmEvent| farm_notice(k, &e, "cashfarm");
        assert_eq!(n(FarmKind::MarketData, FarmEvent::Connected(2)), (2104, "Market data farm connection is OK:cashfarm".into()));
        assert_eq!(n(FarmKind::MarketData, FarmEvent::Connecting(2)), (2119, "Market data farm is connecting:cashfarm".into()));
        assert_eq!(n(FarmKind::MarketData, FarmEvent::IdleClosed(2)).1,
            "Market data farm connection is inactive but should be available upon demand.cashfarm");
        assert_eq!(n(FarmKind::Historical, FarmEvent::IdleClosed(2)).0, 2107);
        assert_eq!(n(FarmKind::Historical, FarmEvent::Lost(2)), (2105, "HMDS data farm connection is broken:cashfarm".into()));
    }
}
