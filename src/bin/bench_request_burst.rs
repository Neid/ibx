//! Benchmark: a burst of contract details requests (ibx#564).
//!
//! Sends N contract details requests for N different stocks in the same
//! instant and measures when each ends, and what the server refuses. Works
//! with the market closed. With BENCH_REQUEST_PACING=0 the requests leave
//! at once, which shows what the servers take; by default they are paced.
//!
//! Env vars:
//!   BENCH_BURST_SIZE     - number of requests (default: 50, at most 100)
//!   BENCH_REQUEST_PACING - 0: the requests are not paced

#[path = "../../bench/bench_harness.rs"]
mod harness;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ibx::bridge::Event;
use ibx::types::*;

use harness::*;

const SYMBOLS: &[&str] = &[
    "AAPL", "MSFT", "AMZN", "GOOGL", "META", "NVDA", "TSLA", "JPM", "V", "JNJ",
    "WMT", "PG", "MA", "UNH", "HD", "DIS", "BAC", "XOM", "KO", "PEP",
    "CSCO", "ORCL", "INTC", "CVX", "MRK", "PFE", "ABT", "T", "VZ", "NKE",
    "CRM", "ADBE", "NFLX", "MCD", "COST", "WFC", "MDT", "BMY", "QCOM", "TXN",
    "HON", "UNP", "IBM", "AMGN", "SBUX", "CAT", "GS", "MS", "BA", "MMM",
    "GE", "LMT", "BLK", "AXP", "SPGI", "PLD", "AMT", "ISRG", "GILD", "BKNG",
    "DE", "SYK", "MDLZ", "ADP", "TMO", "LOW", "C", "SCHW", "CB", "CI",
    "MO", "DUK", "SO", "ZTS", "CL", "BDX", "EOG", "ITW", "APD", "PNC",
    "USB", "TGT", "FDX", "GM", "F", "EMR", "NSC", "AIG", "MET", "PRU",
    "AEP", "D", "EXC", "SRE", "KMB", "GIS", "HSY", "ADM", "SYY", "KR",
];

fn main() {
    let _log = ibx::logging::init(&ibx::logging::LogConfig::from_env());

    let config = BenchConfig::from_env();
    let size = (BenchConfig::env_u32("BENCH_BURST_SIZE", 50) as usize).min(SYMBOLS.len());

    print_header("Bench: Contract Details Burst");
    println!("  Requests:       {}", size);
    println!();

    let session = BenchSession::connect(&config);
    println!("Connected in {:.3}s (account: {})", session.connect_time.as_secs_f64(), session.account_id);
    std::thread::sleep(Duration::from_secs(2));
    while session.event_rx.try_recv().is_ok() {}

    let first: ReqId = 7_000;
    let start = Instant::now();
    for (k, symbol) in SYMBOLS.iter().take(size).enumerate() {
        let _ = session.control_tx.send(ControlCommand::FetchContractDetails {
            req_id: first + k as ReqId,
            con_id: 0,
            symbol: symbol.to_string(),
            sec_type: "STK".into(),
            exchange: "SMART".into(),
            currency: "USD".into(),
            filters: Default::default(),
        });
    }
    let sent = start.elapsed();

    let mut ends = LatencyStats::new(size);
    // The requests in the order their ends came.
    let mut order: Vec<ReqId> = Vec::with_capacity(size);
    let mut errors: HashMap<i64, (i64, String)> = HashMap::new();
    let mut open: std::collections::BTreeSet<ReqId> = (0..size).map(|k| first + k as ReqId).collect();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !open.is_empty() && Instant::now() < deadline {
        match session.event_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::ContractDetailsEnd(req_id)) if open.remove(&req_id) => {
                ends.push(start.elapsed().as_nanos() as u64);
                order.push(req_id);
            }
            Ok(Event::Error { req_id, code, message }) if open.contains(&(req_id as ReqId)) => {
                open.remove(&(req_id as ReqId));
                errors.insert(req_id, (code, message));
            }
            _ => {}
        }
    }

    print_header("Results: Contract Details Burst");
    println!("  {} requests sent in {}", size, format_ns(sent.as_nanos() as u64));
    ends.report("END OF EACH REQUEST (from the first send)");
    let late = order.windows(2).filter(|w| w[1] < w[0]).count();
    println!("  Ends out of the order of the requests: {} of {}", late, order.len());
    println!("  Refused:        {} {:?}", errors.len(), errors.values().take(5).collect::<Vec<_>>());
    println!("  No answer:      {} {:?}", open.len(), open.iter().take(10).collect::<Vec<_>>());
    session.shutdown();
}
