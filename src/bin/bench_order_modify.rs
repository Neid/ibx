//! Benchmark: order modify (amend) latency.
//!
//! Submits a far-from-market GTC limit order, then modifies the price N times,
//! measuring modify→ack latency for each. Cancels at the end.
//!
//! Env vars:
//!   BENCH_CON_ID     - contract ID (default: 756733 = SPY)
//!   BENCH_ITERATIONS - number of modify cycles (default: 20)
//!   BENCH_WARMUP     - warmup ticks before starting (default: 50)

#[path = "../../bench/bench_harness.rs"]
mod harness;

use std::time::{Duration, Instant};

use ibx::bridge::Event;
use ibx::types::*;

use harness::*;

fn main() {
    let _log = ibx::logging::init(&ibx::logging::LogConfig::from_env());

    let config = BenchConfig::from_env();
    let iterations = BenchConfig::env_u32("BENCH_ITERATIONS", 20);
    let warmup_ticks = BenchConfig::env_u32("BENCH_WARMUP", 50);

    print_header("Bench: Order Modify (Amend) Latency");
    println!("  Contract:       {} (con_id={})", config.symbol, config.con_id);
    println!("  Iterations:     {}", iterations);
    println!("  Warmup:         {} ticks", warmup_ticks);
    println!();

    // Connect
    println!("Connecting to IB...");
    let session = BenchSession::connect(&config);
    println!(
        "Connected in {:.3}s (account: {})",
        session.connect_time.as_secs_f64(),
        session.account_id,
    );

    // Subscribe
    session.subscribe(config.con_id, config.symbol);
    let start = Instant::now();
    let instrument = warmup(&session.event_rx, warmup_ticks, start);

    // Submit initial order at $1.00
    // Above the ids of the earlier runs: an id the server has seen is
    // refused as a duplicate (ibx#570).
    let base_order_id = session.first_order_id();
    let mut current_order_id = base_order_id;
    println!("  First order id: {}", base_order_id);

    println!(
        "[{:.3}s] Submitting initial GTC limit BUY 1 @ $1.00...",
        start.elapsed().as_secs_f64(),
    );
    session.send_order(OrderRequest::SubmitLimitGtc {
        order_id: current_order_id,
        instrument,
        side: Side::Buy,
        qty: 1,
        price: 1 * PRICE_SCALE,
        outside_rth: true,
    });
    session.keep_order_id(current_order_id);

    // Wait for initial ack
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > deadline {
            println!("Initial order timed out");
            // It may still reach the server: not left working.
            let left = session.cancel_all(&[current_order_id], Duration::from_secs(10));
            println!("  Orders left working: {} {:?}", left.len(), left);
            session.shutdown();
            std::process::exit(1);
        }
        match session.event_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::OrderUpdate(update)) if update.order_id == current_order_id => {
                if is_ack(update.status) {
                    println!(
                        "[{:.3}s] Initial order acked",
                        start.elapsed().as_secs_f64(),
                    );
                    break;
                }
                if update.status == OrderStatus::Rejected {
                    println!("Initial order REJECTED");
                    session.shutdown();
                    return;
                }
            }
            Ok(_) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                session.shutdown();
                return;
            }
        }
    }

    // Modify N times, alternating price between $1.01 and $1.02
    let mut modify_stats = LatencyStats::new(iterations as usize);

    for i in 0..iterations {
        let new_price = if i % 2 == 0 {
            102 * (PRICE_SCALE / 100) // $1.02
        } else {
            101 * (PRICE_SCALE / 100) // $1.01
        };

        let new_order_id = base_order_id + 1 + i as i64;
        session.keep_order_id(new_order_id);
        // The reports of the order placed or of the modify before come in
        // several messages, the last ones 100 ms or more after the first:
        // wait until the order is quiet, so that the next report is this
        // modify's (ibx#570).
        while session.event_rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
        let modify_time = Instant::now();
        session.send_order(OrderRequest::Modify {
            new_order_id,
            order_id: current_order_id,
            qty: 1,
            kind: OrderKind::Limit { price: new_price },
            tif: b'0',
            attrs: OrderAttrs::default(),
        });

        // Wait for ack on new_order_id
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut acked = false;
        let mut renamed = false;
        let mut statuses: Vec<String> = Vec::new();
        loop {
            if Instant::now() > deadline {
                println!("  Iteration {} modify timed out", i + 1);
                break;
            }
            match session.event_rx.recv_timeout(Duration::from_secs(1)) {
                // The server reports the replaced order under the id it
                // had; an engine that renames it reports the new id.
                Ok(Event::OrderUpdate(update)) if update.order_id == new_order_id || update.order_id == current_order_id => {
                    renamed = update.order_id == new_order_id;
                    statuses.push(format!("{:?}@{}ms", update.status, modify_time.elapsed().as_millis()));
                    // The server's report of the replaced order: a working
                    // status. A status the engine gives itself when the
                    // modify leaves is not the acknowledgement.
                    if matches!(update.status, OrderStatus::PreSubmitted | OrderStatus::Submitted) {
                        let ns = (Instant::now() - modify_time).as_nanos() as u64;
                        modify_stats.push(ns);
                        println!(
                            "[{:.3}s] Modify {}/{}: {}",
                            start.elapsed().as_secs_f64(),
                            i + 1,
                            iterations,
                            format_ns(ns),
                        );
                        acked = true;
                        break;
                    }
                }
                Ok(Event::CancelReject(reject)) if reject.order_id == current_order_id => {
                    println!("  Modify rejected (reason={})", reject.reason_code);
                    break;
                }
                Ok(_) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }

        if std::env::var_os("BENCH_VERBOSE").is_some() {
            println!("    statuses: {}", statuses.join(", "));
        }
        if acked && renamed {
            current_order_id = new_order_id;
        }

    }

    // Cancel the order
    println!(
        "[{:.3}s] Cancelling order...",
        start.elapsed().as_secs_f64(),
    );
    // The order under its last id, and the id of a modify that was not
    // acknowledged: whichever the server knows is cancelled.
    let mut left = session.cancel_all(&[current_order_id], Duration::from_secs(30));
    if !left.is_empty() {
        let ids: Vec<i64> = (base_order_id..=base_order_id + iterations as i64).collect();
        left = session.cancel_all(&ids, Duration::from_secs(5));
    }

    // Report
    print_header("Results: Order Modify Latency");
    modify_stats.report("MODIFY → ACK (server report of the replaced order)");
    println!();
    println!("  Order ids:           {} to {}", base_order_id, base_order_id + iterations as i64);
    println!("  Orders left working: {} {:?}", left.len(), left);

    session.shutdown();
    if !left.is_empty() {
        std::process::exit(1);
    }
}
