//! The FA data exchange with the server (ibx#507), as the reference 10.51
//! makes it (not observed yet: no FA account). On an FA session the
//! groups are asked once after the logon (the profiles too, apart, in the
//! classic mode); a replace deletes the old objects and saves the new
//! ones, and is answered when every save of a kind is answered. An order
//! on a group waits while a save is not answered, and is refused with
//! 10228 when the save fails.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::bridge::SharedState;
use crate::config::chrono_free_timestamp;
use crate::control::fa::{self, FaData, SaveBatch};
use crate::protocol::connection::Connection;
use crate::types::OrderId;

use super::HeartbeatState;

/// The saves of one kind of one replace.
struct Batch {
    req_id: i64,
    profiles: bool,
    remaining: usize,
    failed: bool,
}

/// The FA exchange of the session, and the groups and profiles the
/// orders need.
#[derive(Default)]
pub(crate) struct FaState {
    asked: bool,
    next_id: u64,
    /// The "get all" requests by id: true for the profiles.
    downloads: HashMap<String, bool>,
    /// The saves not answered, by id: their batch.
    saves: HashMap<String, usize>,
    batches: Vec<Batch>,
    /// A save of the round failed: the orders that waited are refused.
    round_failed: bool,
    /// The link was lost: the saves not answered fail.
    lost: bool,
    /// The orders on a group that wait for the saves, and those to
    /// refuse with 10228.
    waiting: HashSet<OrderId>,
    failed: HashSet<OrderId>,
    /// The unified mode of the logon.
    pub(crate) unified: bool,
    /// The stored method of each group (`None`: the group has none), and
    /// the profiles' names.
    pub(crate) groups: HashMap<String, Option<String>>,
    pub(crate) profiles: HashSet<String>,
}

impl FaState {
    fn id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    /// The groups and profiles of the session's data, for the orders.
    pub(crate) fn refresh(&mut self, data: &FaData) {
        self.groups = data.groups.iter().map(|g| (g.name.clone(), g.method.clone().filter(|m| !m.is_empty()))).collect();
        self.profiles = data.profiles.iter().map(|p| p.name.clone()).collect();
    }

    /// A save is not answered yet.
    pub(crate) fn pending(&self) -> bool {
        !self.saves.is_empty()
    }

    /// An order on a group while a save is not answered: it waits.
    pub(crate) fn wait(&mut self, order_id: OrderId) {
        self.waiting.insert(order_id);
    }

    /// An order that waited for a save that failed.
    pub(crate) fn take_failed(&mut self, order_id: OrderId) -> bool {
        self.failed.remove(&order_id)
    }

    /// A save not answered, for a test.
    #[cfg(test)]
    pub(crate) fn pend_for_test(&mut self) {
        self.saves.insert("test".into(), 0);
    }
}

fn send(conn: &mut Connection, hb: &mut HeartbeatState, fields: &[(u32, &str)]) -> bool {
    let ts = chrono_free_timestamp();
    let mut all: Vec<(u32, &str)> = Vec::with_capacity(fields.len() + 2);
    all.push((35, "U"));
    all.push((52, &ts));
    all.extend_from_slice(fields);
    let sent = conn.send_fix(&all).is_ok();
    if sent {
        hb.last_ccp_sent = Instant::now();
    }
    sent
}

fn has_feature(shared: &SharedState, feature: &str) -> bool {
    shared.reference.logon_features().split(',').any(|f| f == feature)
}

/// Once per poll of the auth link on an FA session: the logon download,
/// asked once, and the saves lost with the link. True when the orders
/// that waited for a save are to be handled again.
pub(crate) fn tick(fa: &mut FaState, conn: &mut Option<Connection>, hb: &mut HeartbeatState, shared: &SharedState, connected: bool) -> bool {
    if fa.lost {
        fa.lost = false;
        let pending: Vec<usize> = fa.saves.drain().map(|(_, b)| b).collect();
        for index in pending {
            answer_save(fa, index, Some(fa::CONNECTION_FAILED.to_string()), shared);
        }
        return end_round(fa);
    }
    if fa.asked || !connected {
        return false;
    }
    let Some(conn) = conn.as_mut() else { return false };
    fa.asked = true;
    fa.unified = has_feature(shared, fa::UNIFIED_FEATURE);
    for profiles in [false, true] {
        if profiles && fa.unified {
            continue;
        }
        let id = fa.id();
        let kind = if profiles { "Profile" } else { "Group" };
        if send(conn, hb, &[(6040, "116"), (6905, "5"), (6158, &id), (6906, kind)]) {
            log::info!("FA {} asked ({})", kind, id);
            fa.downloads.insert(id, profiles);
        }
    }
    false
}

/// The link was lost: the saves not answered fail at the next tick.
pub(crate) fn connection_lost(fa: &mut FaState) {
    if !fa.saves.is_empty() {
        fa.lost = true;
    }
}

/// A replace (ibx#507): the deletes and the saves of each batch. The
/// client counted the saves as pending; the groups and profiles of the
/// orders are the replaced ones.
pub(crate) fn save(fa: &mut FaState, req_id: i64, batches: Vec<SaveBatch>, conn: &mut Option<Connection>, hb: &mut HeartbeatState,
    shared: &SharedState, connected: bool)
{
    fa.refresh(&shared.reference.fa_data());
    for batch in batches {
        if let Some(conn) = conn.as_mut().filter(|_| connected) {
            for name in &batch.deletes {
                let id = fa.id();
                send(conn, hb, &[(6040, "116"), (6905, "4"), (6158, &id), (6906, batch.kind), (6907, name)]);
            }
        }
        if batch.saves.is_empty() {
            continue;
        }
        let index = fa.batches.len();
        fa.batches.push(Batch { req_id, profiles: batch.profiles, remaining: batch.saves.len(), failed: false });
        for (name, xml) in &batch.saves {
            let id = fa.id();
            let sent = conn.as_mut().filter(|_| connected)
                .is_some_and(|conn| send(conn, hb, &[(6040, "116"), (6905, "3"), (6158, &id), (6906, batch.kind), (6907, name), (6118, xml)]));
            if sent {
                fa.saves.insert(id, index);
            } else {
                answer_save(fa, index, Some(fa::CONNECTION_FAILED.to_string()), shared);
            }
        }
    }
}

/// An answer of the FA exchange. True when the orders that
/// waited for a save are to be handled again.
pub(crate) fn answer(fa: &mut FaState, parsed: &HashMap<u32, &str>, shared: &SharedState) -> bool {
    let id = parsed.get(&6158).copied().unwrap_or("");
    let error = parsed.get(&58).copied().unwrap_or("");
    if let Some(profiles) = fa.downloads.remove(id) {
        if !error.is_empty() {
            log::warn!("FA download {} refused: {}", id, error);
            return false;
        }
        let Some(xml) = parsed.get(&6118) else {
            log::warn!("FA download {}: no document", id);
            return false;
        };
        let mut data = shared.reference.fa_data();
        if profiles {
            data.profiles = fa::server_profiles(xml);
        } else {
            let linked: Vec<&str> = parsed.get(&8395).map(|v| v.split(',').map(str::trim).collect()).unwrap_or_default();
            let (groups, profiles) = fa::server_groups(xml, fa.unified, &linked, has_feature(shared, fa::KEEP_EMPTY_FEATURE));
            data.groups = groups;
            if fa.unified {
                data.profiles = profiles;
            }
        }
        log::info!("FA data: {} group(s), {} profile(s)", data.groups.len(), data.profiles.len());
        fa.refresh(&data);
        return false;
    }
    let Some(index) = fa.saves.remove(id) else { return false };
    answer_save(fa, index, (!error.is_empty()).then(|| error.to_string()), shared);
    end_round(fa)
}

fn answer_save(fa: &mut FaState, index: usize, error: Option<String>, shared: &SharedState) {
    {
        let mut data = shared.reference.fa_data();
        let pending = if fa.batches[index].profiles { &mut data.pending_profiles } else { &mut data.pending_groups };
        *pending = pending.saturating_sub(1);
    }
    let batch = &mut fa.batches[index];
    if batch.failed {
        return;
    }
    match error {
        Some(text) => {
            batch.failed = true;
            fa.round_failed = true;
            log::warn!("FA save of request {} failed: {}", batch.req_id, text);
            shared.orders.push_order_error(batch.req_id, 10229, crate::control::historical::join_error_text(fa::SAVING_ERROR, &text));
        }
        None => {
            batch.remaining -= 1;
            if batch.remaining == 0 {
                shared.reference.push_fa_saved(batch.req_id);
            }
        }
    }
}

/// Every save answered: the orders that waited go on, or are refused
/// when a save failed.
fn end_round(fa: &mut FaState) -> bool {
    if !fa.saves.is_empty() {
        return false;
    }
    fa.batches.clear();
    let waited = std::mem::take(&mut fa.waiting);
    if std::mem::take(&mut fa.round_failed) {
        fa.failed.extend(waited.iter().copied());
    }
    !waited.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::fix;

    /// The frames `n` the server end reads.
    fn frames(server: &mut crate::protocol::connection::MemTransport, n: usize) -> Vec<HashMap<u32, String>> {
        use std::io::Read;
        server.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let mut buf = vec![0u8; 65536];
        let mut len = 0;
        loop {
            let out: Vec<HashMap<u32, String>> = buf[..len].split(|&b| b == fix::SOH)
                .filter_map(|f| {
                    let (t, v) = std::str::from_utf8(f).ok()?.split_once('=')?;
                    Some((t.parse::<u32>().ok()?, v.to_string()))
                })
                .fold(Vec::new(), |mut acc: Vec<HashMap<u32, String>>, (t, v)| {
                    if t == 8 { acc.push(HashMap::new()); }
                    if let Some(last) = acc.last_mut() { last.insert(t, v); }
                    acc
                })
                .into_iter().filter(|f| f.contains_key(&10)).collect();
            if out.len() >= n {
                return out;
            }
            let read = server.read(&mut buf[len..]).unwrap();
            assert!(read > 0, "connection closed");
            len += read;
        }
    }

    fn reply<'a>(id: &'a str, error: &'a str, xml: Option<&'a str>) -> HashMap<u32, &'a str> {
        let mut m = HashMap::from([(6040, "117"), (6158, id)]);
        if !error.is_empty() { m.insert(58, error); }
        if let Some(xml) = xml { m.insert(6118, xml); }
        m
    }

    #[test]
    fn the_download_the_saves_and_their_answers() {
        let (client, mut server) = crate::protocol::connection::mem_pair();
        let mut conn = Some(Connection::new_mem(client));
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();
        shared.reference.set_logon_features("FAONEGRP");
        shared.reference.set_managed_accounts(vec!["U1".into()]);
        let mut fa = FaState::default();

        // The logon download: the groups only in the unified mode, once.
        assert!(!tick(&mut fa, &mut conn, &mut hb, &shared, true));
        assert!(!tick(&mut fa, &mut conn, &mut hb, &shared, true));
        let asked = frames(&mut server, 1);
        assert_eq!(asked.len(), 1);
        assert_eq!((asked[0][&6040].as_str(), asked[0][&6905].as_str(), asked[0][&6906].as_str()), ("116", "5", "Group"));
        assert!(!asked[0].contains_key(&1), "no account on a group request");
        let id = asked[0][&6158].clone();
        let xml = "<ListOfGroups><Group><name>G</name><ListOfAccts varName=\"list\"><String>U1</String></ListOfAccts>\
            <defaultMethod>NetLiq</defaultMethod></Group></ListOfGroups>";
        answer(&mut fa, &reply(&id, "", Some(xml)), &shared);
        assert_eq!(shared.reference.fa_data().groups[0].name, "G");
        assert_eq!(fa.groups.get("G"), Some(&Some("NetLiq".to_string())));

        // A replace: the delete, then the save; answered at the last save.
        let batches = vec![SaveBatch { kind: "Group", profiles: false, deletes: vec!["G".into()],
            saves: vec![("A".into(), "<a/>".into()), ("B".into(), "<b/>".into())] }];
        shared.reference.fa_data().pending_groups = 2;
        save(&mut fa, 9, batches, &mut conn, &mut hb, &shared, true);
        let sent = frames(&mut server, 3);
        let ops: Vec<(String, String)> = sent.iter().map(|f| (f[&6905].clone(), f[&6907].clone())).collect();
        assert_eq!(ops, [("4".into(), "G".into()), ("3".into(), "A".into()), ("3".into(), "B".into())]);
        assert_eq!(sent[1][&6118], "<a/>");
        assert!(fa.pending());
        fa.wait(77);
        assert!(!answer(&mut fa, &reply(&sent[0][&6158], "", None), &shared), "a delete answers nothing");
        assert!(!answer(&mut fa, &reply(&sent[1][&6158], "", None), &shared));
        assert!(shared.reference.drain_fa_saved().is_empty());
        assert!(answer(&mut fa, &reply(&sent[2][&6158], "", None), &shared), "the orders that waited go on");
        assert_eq!(shared.reference.drain_fa_saved(), [9]);
        assert_eq!(shared.reference.fa_data().pending_groups, 0);
        assert!(!fa.take_failed(77));

        // A save refused: 10229 with the server's text, once; the orders
        // that waited are refused.
        let batches = vec![SaveBatch { kind: "Group", profiles: false, deletes: vec![],
            saves: vec![("A".into(), "<a/>".into()), ("B".into(), "<b/>".into())] }];
        save(&mut fa, 10, batches, &mut conn, &mut hb, &shared, true);
        let sent = frames(&mut server, 2);
        fa.wait(78);
        answer(&mut fa, &reply(&sent[0][&6158], "Bad group", None), &shared);
        assert!(answer(&mut fa, &reply(&sent[1][&6158], "Other", None), &shared));
        assert_eq!(shared.orders.drain_order_errors(), [(10, 10229, "FA data saving error:Bad group".to_string())]);
        assert!(shared.reference.drain_fa_saved().is_empty());
        assert!(fa.take_failed(78));

        // The link lost before the answer: 10229 "Connection failed".
        let batches = vec![SaveBatch { kind: "Group", profiles: true, deletes: vec![], saves: vec![("P".into(), "<p/>".into())] }];
        save(&mut fa, 11, batches, &mut conn, &mut hb, &shared, true);
        connection_lost(&mut fa);
        tick(&mut fa, &mut conn, &mut hb, &shared, false);
        assert_eq!(shared.orders.drain_order_errors(), [(11, 10229, "FA data saving error:Connection failed".to_string())]);
        assert!(!fa.pending());
    }
}
