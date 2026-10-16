//! ibx#614 probe through the Rust client, paper account only: a parent BUY
//! 1 AAPL LMT 100 (far from the market) held with transmit off, then a SELL
//! MKT beta hedge child of it, as cases A and B of the reference runs of
//! 15/10/2026: without a max size (10364, the parent stays held), with one
//! (both sent). Prints the errors and order statuses. Nothing can fill.
//!
//! Paper: IB_USERNAME, IB_PASSWORD.
use std::env;
use std::time::{Duration, Instant};

use ibx::api::client::{Contract, EClient, EClientConfig, Order};
use ibx::api::wrapper::Wrapper;

struct Log {
    t0: Instant,
}

impl Log {
    fn line(&self, text: String) {
        println!("   +{:6} ms  {text}", self.t0.elapsed().as_millis());
    }
}

impl Wrapper for Log {
    fn error(&mut self, req_id: i64, code: i64, text: &str, _: &str) {
        if !matches!(code, 2104 | 2106 | 2107 | 2108 | 2119 | 2158) {
            self.line(format!("error {req_id} {code} {text}"));
        }
    }
    fn order_status(&mut self, order_id: i64, status: &str, _: f64, _: f64, _: f64, perm_id: i64, parent_id: i64,
        _: f64, _: i64, _: &str, _: f64) {
        self.line(format!("orderStatus {order_id} {status} perm={} parent={parent_id}", perm_id != 0));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = EClient::connect(&EClientConfig {
        username: env::var("IB_USERNAME")?, password: env::var("IB_PASSWORD")?,
        host: env::var("IB_HOST").unwrap_or_else(|_| "cdc1.ibllc.com".into()),
        paper: true, core_id: None,
        client_id: 198,
    })?;
    if !client.account_id.starts_with("DU") {
        client.disconnect();
        return Err("not a paper account: nothing sent".into());
    }
    let pump = |client: &EClient, w: &mut Log, secs: f64| {
        let until = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < until {
            client.process_msgs(w);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let mut w = Log { t0: Instant::now() };
    pump(&client, &mut w, 2.0);
    let aapl = Contract { symbol: "AAPL".into(), sec_type: "STK".into(), exchange: "SMART".into(), currency: "USD".into(), ..Default::default() };
    for (case, max_size) in [("A", i32::MAX), ("B", 100)] {
        let parent_id = client.next_order_id();
        let hedge_id = parent_id + 1;
        let parent = Order {
            action: "BUY".into(), order_type: "LMT".into(), total_quantity: 1.0, lmt_price: 100.0, tif: "DAY".into(),
            outside_rth: true, transmit: false, ..Default::default()
        };
        let hedge = Order {
            action: "SELL".into(), order_type: "MKT".into(), total_quantity: 0.0, tif: "DAY".into(), parent_id,
            hedge_type: "B".into(), hedge_param: "1".into(), hedge_max_size: max_size, transmit: true, ..Default::default()
        };
        w.t0 = Instant::now();
        println!("== case {case}, parent {parent_id}, hedge {hedge_id}");
        client.place_order(parent_id, &aapl, &parent)?;
        pump(&client, &mut w, 0.5);
        client.place_order(hedge_id, &aapl, &hedge)?;
        pump(&client, &mut w, 5.0);
        let _ = client.cancel_order(parent_id, "");
        pump(&client, &mut w, 1.0);
        let _ = client.cancel_order(hedge_id, "");
        pump(&client, &mut w, 2.0);
    }
    client.disconnect();
    Ok(())
}
