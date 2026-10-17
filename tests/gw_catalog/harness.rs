//! The engine on in-memory connections, driven through the API client, with
//! the frames it sends to the server and the callbacks the client gets.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use ibx::api::client::{Contract, EClient, Order};
use ibx::api::wrapper::tests::RecordingWrapper;
use ibx::bridge::SharedState;
use ibx::engine::hot_loop::HotLoop;
use ibx::test_support::{parse_fields, Fields, Peer};
use ibx::types::{ControlCommand, InstrumentId, OrderRequest};

pub const AAPL: i64 = 265598;

pub fn aapl() -> Contract {
    Contract {
        con_id: AAPL, symbol: "AAPL".into(), sec_type: "STK".into(),
        exchange: "SMART".into(), currency: "USD".into(), ..Default::default()
    }
}

/// A limit order of the API, the base of the cases.
pub fn lmt(action: &str, qty: f64, price: f64) -> Order {
    Order {
        action: action.into(), total_quantity: qty, order_type: "LMT".into(),
        lmt_price: price, tif: "DAY".into(), ..Default::default()
    }
}

pub struct Engine {
    pub client: EClient,
    pub shared: Arc<SharedState>,
    control: ibx::engine::park::ControlSender,
    ccp: Peer,
    /// The market data farm, open for the whole test: a closed one is a
    /// lost link (2103) among the errors.
    _farm: Peer,
    handle: Option<JoinHandle<()>>,
    /// Every order message (new order, replace, cancel) the server got so far.
    orders: Vec<Fields>,
}

impl Engine {
    /// Wait until a held order is known as a parent: the lookups of its
    /// placeOrder answered (ibx#718). A hedge placed before is refused as
    /// under no parent, as the reference does.
    pub fn wait_held_known(&self, order_id: i64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.shared.orders.is_held_unknown(order_id.into()) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// The engine with a signed auth link to a scripted server, and an API
    /// client of id 39 on it.
    pub fn start() -> Self {
        let shared = Arc::new(SharedState::new());
        shared.reference.set_api_client_id(39);
        let (farm_conn, farm) = Peer::pair();
        let (mut ccp_conn, mut ccp) = Peer::pair();
        let mac_key: Vec<u8> = (1..=20).collect();
        ccp_conn.set_keys(mac_key.clone(), (0..16).collect(), mac_key, (16..32).collect());
        ccp.sign_like(&ccp_conn);
        let (mut engine, control) = HotLoop::with_connections(
            shared.clone(), None, "DUXXXXXXX".into(), farm_conn, ccp_conn, None, None);
        let handle = std::thread::spawn(move || engine.run());
        let client = EClient::from_parts(shared.clone(), control.clone(), std::thread::spawn(|| {}), "DUXXXXXXX".into());
        Self { client, shared, control, ccp, _farm: farm, handle: Some(handle), orders: Vec::new() }
    }

    /// Register a contract with the engine (as the API client does on its
    /// first order), with its currency; its instrument id.
    pub fn register(&self, contract: &Contract) -> InstrumentId {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.control.send(ControlCommand::RegisterInstrument {
            con_id: contract.con_id, symbol: contract.symbol.clone(), sec_type: contract.sec_type.clone(),
            exchange: contract.exchange.clone(), reply_tx: Some(reply_tx),
        }).unwrap();
        let id = reply_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        self.control.send(ControlCommand::SetInstrumentCurrency {
            con_id: contract.con_id, currency: contract.currency.clone(),
        }).unwrap();
        id
    }

    /// Hand an order request to the engine, as the API client does.
    pub fn send(&self, req: OrderRequest) {
        self.control.send(ControlCommand::Order(req)).unwrap();
    }

    fn read(&mut self) {
        for m in self.ccp.messages() {
            let fields = parse_fields(&m);
            if fields.iter().any(|(t, v)| *t == 35 && matches!(v.as_str(), "D" | "G" | "F")) {
                self.orders.push(fields);
            }
        }
    }

    /// The order messages the server got, once there are `n` of them (or
    /// after 5 s).
    pub fn orders(&mut self, n: usize) -> Vec<Fields> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.read();
            if self.orders.len() >= n || Instant::now() >= deadline {
                return self.orders.clone();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Answer the engine's request of the order preset of `sec_type` once it
    /// is sent (or after 5 s), as the server does: with the preset's time in
    /// force, none when `tif` is empty (ibx#688).
    pub fn answer_preset(&mut self, sec_type: &str, tif: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut asked = false;
        while !asked && Instant::now() < deadline {
            for m in self.ccp.messages() {
                let fields = parse_fields(&m);
                asked |= field(&fields, 35) == Some("U") && field(&fields, 6040) == Some("193");
                if fields.iter().any(|(t, v)| *t == 35 && matches!(v.as_str(), "D" | "G" | "F")) {
                    self.orders.push(fields);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(asked, "the preset of {sec_type} is asked");
        let kind = format!("s={sec_type}");
        let mut reply = vec![(35, "U"), (6040, "194"), (8166, "G"), (8168, kind.as_str())];
        if !tif.is_empty() {
            reply.push((4026, tif));
        }
        self.ccp.send_fix(&reply);
    }

    /// The callbacks the API client got since the last call.
    pub fn callbacks(&self) -> Vec<String> {
        let mut w = RecordingWrapper::default();
        self.client.process_msgs(&mut w);
        w.events
    }

    /// The errors (`(id, code, text)`) the API client got since the last
    /// call.
    pub fn errors(&self) -> Vec<(i64, i64, String)> {
        self.callbacks().iter().filter_map(|e| {
            let rest = e.strip_prefix("error:")?;
            let mut parts = rest.splitn(3, ':');
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.to_string()))
        }).collect()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.control.send(ControlCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The value of a field.
pub fn field(fields: &Fields, tag: u32) -> Option<&str> {
    fields.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_str())
}
