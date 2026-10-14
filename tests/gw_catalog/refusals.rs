//! Local refusals of an order (ibx#485 item 2): one test per gateway rule
//! of `order_local_rules.csv` that ibx implements. The API order goes in
//! through `place_order` (or `cancel_order`), the error out is the rule's
//! code and text, and the server gets no order message for it: a valid
//! order placed after it is the only one the in-memory server reads.
//!
//! The rules ibx does not implement are rows of the coverage matrix marked
//! open with their issue.

use ibx::api::client::{Contract, Order, TagValue};
use ibx::api::types::ComboLeg;
use ibx::types::OrderCondition;

use super::catalog::{local_rule, matches_template};
use super::harness::{aapl, field, lmt, Engine};

/// The order id of the refused order, and of the valid one after it.
const REFUSED: i64 = 7;
const NEXT: i64 = 900;

/// `engine` refused order `id` with `rule`'s code and text (its `%s` filled
/// with any value) and sent nothing for it.
fn assert_refused(engine: &mut Engine, id: i64, rule: &str) {
    let rule = local_rule(rule);
    // The engine's refusals come from its thread.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut errors = Vec::new();
    while !errors.iter().any(|e: &(i64, i64, String)| e.0 == id) && std::time::Instant::now() < deadline {
        errors.extend(engine.errors());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let mine: Vec<&(i64, i64, String)> = errors.iter().filter(|e| e.0 == id).collect();
    assert!(
        mine.iter().any(|(_, code, text)| *code == rule.api_code && (matches_template(&rule.text, text, false)
            // The table is 10.40's; 10.51 adds the order id to 103 (ibx#625).
            || (rule.api_code == 103 && *text == format!("{}: {}", rule.text, id)))),
        "rule {} (gateway code {}): want {} {:?}, got {:?}", rule.id, rule.code, rule.api_code, rule.text, errors,
    );
    // Nothing sent for the refused order: the next order is the first the
    // server reads after it.
    let before = engine.orders(0).len();
    engine.client.place_order(NEXT, &aapl(), &lmt("BUY", 1.0, 100.0)).unwrap();
    let frames = engine.orders(before + 1);
    let after = &frames[before..];
    assert!(!after.is_empty(), "the valid order after it is sent");
    let id_text = id.to_string();
    let refused: Vec<String> = frames.iter()
        .filter(|f| field(f, 6121) == Some(id_text.as_str())
            || field(f, 11).is_some_and(|c| c.split('.').next() == Some(id_text.as_str())))
        .map(|f| ibx::test_support::to_pipe(f))
        .collect();
    assert!(refused.is_empty(), "rule {}: frames sent for the refused order: {refused:?}", rule.id);
}

/// Place `order` on `contract` and check `rule`.
fn refused(rule: &str, contract: &Contract, order: &Order) {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, contract, order).expect("a refusal is an error callback, not a call error");
    assert_refused(&mut engine, REFUSED, rule);
}

fn order(order_type: &str) -> Order {
    Order { order_type: order_type.into(), ..lmt("BUY", 1.0, 100.0) }
}

// ── The reading of the order: 320 with the rule text, or the date codes ──

// A date that cannot be read is refused with the table's text under the
// code 391 (ibx#548, paper 11/10/2026: "not a date"); 343 is the code of a
// zone that is not the contract's.
#[test]
fn good_till_date_not_a_date() {
    let mut engine = Engine::start();
    let order = Order { tif: "GTD".into(), good_till_date: "next week".into(), ..order("LMT") };
    engine.client.place_order(REFUSED, &aapl(), &order).unwrap();
    let rule = local_rule("343-1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut errors = Vec::new();
    while errors.is_empty() && std::time::Instant::now() < deadline {
        errors.extend(engine.errors().into_iter().filter(|e| e.0 == REFUSED));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(errors.iter().any(|(_, code, text)| *code == 391 && matches_template(&rule.text, text, false)), "{errors:?}");
}

// A start time that cannot be read is left out by a current client's
// reference, and the order is sent (ibx#594, paper 11/10/2026); a time
// alone is today's (ibx#598). The table's text is the refusal of a zone
// that is not the contract's.
#[test]
fn good_after_time_not_a_date() {
    let mut engine = Engine::start();
    engine.shared.reference.cache_time_zone_id(aapl().con_id, "US/Eastern");
    engine.client.place_order(REFUSED, &aapl(), &Order { good_after_time: "20261230 10:00:00 Asia/Tokyo".into(), ..order("LMT") }).unwrap();
    assert_refused(&mut engine, REFUSED, "337-1");
    let mut engine = Engine::start();
    engine.client.place_order(6, &aapl(), &Order { good_after_time: "15:30:00".into(), ..order("LMT") }).unwrap();
    assert_eq!(field(&engine.orders(1)[0], 6121), Some("6"), "a time alone is sent");
    engine.client.place_order(REFUSED, &aapl(), &Order { good_after_time: "after lunch".into(), ..order("LMT") }).unwrap();
    let frames = engine.orders(2);
    assert_eq!(field(&frames[1], 6121), Some("7"), "the order is sent");
    assert!(engine.errors().iter().all(|e| e.0 != REFUSED));
}

#[test]
fn negative_discretionary_amount() {
    refused("168-1", &aapl(), &Order { discretionary_amt: -0.5, ..order("LMT") });
}

#[test]
fn trailing_percent_with_a_trailing_amount() {
    refused("10060-1", &aapl(), &Order {
        action: "SELL".into(), trailing_percent: 1.0, aux_price: 1.0, ..order("TRAIL")
    });
}

#[test]
fn time_condition_not_a_time() {
    refused("10314-1", &aapl(), &Order {
        conditions: vec![OrderCondition::Time { time: "tomorrow".into(), is_more: true }],
        ..order("LMT")
    });
}

fn bag(legs: Vec<ComboLeg>) -> Contract {
    Contract {
        con_id: 28812380, symbol: "SPY,QQQ".into(), sec_type: "BAG".into(), exchange: "SMART".into(),
        currency: "USD".into(), combo_legs: legs, ..Default::default()
    }
}

fn leg(con_id: i64, action: &str) -> ComboLeg {
    ComboLeg { con_id, ratio: 1, action: action.into(), exchange: "SMART".into(), exempt_code: -1, ..Default::default() }
}

#[test]
fn per_leg_prices_not_one_per_leg() {
    refused("10057-1", &bag(vec![leg(756733, "BUY"), leg(320227571, "SELL")]),
        &Order { order_combo_legs: vec![1.0], ..order("LMT") });
}

// Not observed yet (ibx#688): the reference is expected to answer this
// rule at its first checks (321); ibx answers 320 when it reads the
// request.
#[test]
#[ignore = "ibx#688: 321 expected, ibx sends 320; to observe"]
fn unknown_combo_routing_tag() {
    refused("10027-1", &bag(vec![leg(756733, "BUY"), leg(320227571, "SELL")]), &Order {
        smart_combo_routing_params: vec![TagValue { tag: "Bogus".into(), value: "1".into() }],
        ..order("LMT")
    });
}

// ── The first checks: 321 with the rule text ──

#[test]
fn combo_without_legs() {
    refused("314-1", &bag(Vec::new()), &order("LMT"));
}

#[test]
fn smart_combo_in_a_currency_without_its_conid() {
    // The logon gave no smart combo conId for any currency.
    refused("10011-1", &bag(vec![leg(756733, "BUY"), leg(320227571, "SELL")]), &order("LMT"));
}

#[test]
fn quantity_out_of_range() {
    refused("355-1", &aapl(), &Order { total_quantity: 2e9, ..order("LMT") });
}

#[test]
fn trail_limit_without_a_stop_price() {
    refused("321-2", &aapl(), &Order {
        action: "SELL".into(), aux_price: 1.0, lmt_price_offset: 0.5, lmt_price: f64::MAX, trail_stop_price: f64::MAX,
        ..order("TRAIL LIMIT")
    });
}

#[test]
fn trigger_method_not_the_gateways() {
    refused("146-1", &aapl(), &Order {
        action: "SELL".into(), aux_price: 90.0, trigger_method: 5, ..order("STP")
    });
}

#[test]
fn what_if_without_transmit() {
    refused("413-1", &aapl(), &Order { what_if: true, transmit: false, ..order("LMT") });
}

#[test]
fn trailing_percent_above_100() {
    refused("10061-1", &aapl(), &Order {
        action: "SELL".into(), trailing_percent: 150.0, ..order("TRAIL")
    });
}

#[test]
fn trail_limit_with_both_limit_fields() {
    refused("321-3", &aapl(), &Order {
        action: "SELL".into(), aux_price: 1.0, lmt_price: 99.0, lmt_price_offset: 0.5, trail_stop_price: 99.5,
        ..order("TRAIL LIMIT")
    });
}

#[test]
fn midprice_outside_regular_hours() {
    refused("10210-1", &aapl(), &Order { outside_rth: true, ..order("MIDPRICE") });
}

#[test]
fn customer_account_without_the_account_config() {
    refused("145-2", &aapl(), &Order { customer_account: "CUST1".into(), ..order("LMT") });
}

#[test]
fn professional_customer_without_the_account_config() {
    refused("145-3", &aapl(), &Order { professional_customer: true, ..order("LMT") });
}

#[test]
fn short_side_without_the_logon_flags() {
    refused("321-1", &aapl(), &Order { action: "SSHORT".into(), ..order("LMT") });
}

/// An institutional session (the logon's super-user flag) for the short
/// sale slot rules.
fn short_sale(rule: &str, slot: i32, location: &str, exempt_code: i32) {
    let mut engine = Engine::start();
    engine.shared.reference.set_short_sale_flags(true, false);
    let order = Order {
        action: "SSHORT".into(), short_sale_slot: slot, designated_location: location.into(), exempt_code,
        ..order("LMT")
    };
    engine.client.place_order(REFUSED, &aapl(), &order).unwrap();
    assert_refused(&mut engine, REFUSED, rule);
}

#[test]
fn short_sale_slot_not_1_or_2() {
    short_sale("347-2", 3, "", -1);
}

#[test]
fn short_sale_exempt_code_on_a_short_sale() {
    short_sale("494-1", 1, "", 1);
}

#[test]
fn short_sale_slot_1_with_a_location() {
    short_sale("353-1", 1, "XYZ", -1);
}

#[test]
fn short_sale_slot_2_without_a_location() {
    short_sale("352-1", 2, "", -1);
}

#[test]
fn short_sale_slot_on_a_non_institutional_session() {
    // The omnibus flag alone: a short side is allowed, a slot is not.
    let mut engine = Engine::start();
    engine.shared.reference.set_short_sale_flags(false, true);
    let order = Order { action: "SSHORT".into(), short_sale_slot: 1, ..order("LMT") };
    engine.client.place_order(REFUSED, &aapl(), &order).unwrap();
    assert_refused(&mut engine, REFUSED, "346-1");
}

// ── The API order build ──

#[test]
fn fractional_quantity() {
    refused("10243-1", &aapl(), &Order { total_quantity: 1.5, ..order("LMT") });
}

// ── The order id flow ──

#[test]
fn order_id_zero() {
    let mut engine = Engine::start();
    engine.client.place_order(0, &aapl(), &order("LMT")).expect("a refusal is an error callback");
    assert_refused(&mut engine, 0, "10149-1");
}

#[test]
fn a_new_order_id_below_the_highest_used() {
    let mut engine = Engine::start();
    engine.client.place_order(50, &aapl(), &order("LMT")).unwrap();
    assert_eq!(engine.orders(1).len(), 1, "the first order is sent");
    engine.client.place_order(40, &aapl(), &order("LMT")).unwrap();
    assert_refused(&mut engine, 40, "103-1");
}

// ── Cancel ──

#[test]
fn cancel_of_an_unknown_order() {
    let mut engine = Engine::start();
    engine.client.cancel_order(REFUSED, "").unwrap();
    assert_refused(&mut engine, REFUSED, "10147-1");
}

#[test]
fn cancel_of_an_order_with_a_cancel_pending() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &order("LMT")).unwrap();
    engine.client.cancel_order(REFUSED, "").unwrap();
    let frames = engine.orders(2);
    assert_eq!(frames.iter().filter(|f| field(f, 35) == Some("F")).count(), 1, "the first cancel is sent");
    engine.client.cancel_order(REFUSED, "").unwrap();
    let rule = local_rule("10148-1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let errors = loop {
        let errors = engine.errors();
        if !errors.is_empty() || std::time::Instant::now() >= deadline {
            break errors;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(errors.iter().any(|(id, code, text)| *id == REFUSED && *code == rule.api_code
        && matches_template(&rule.text, text, false)), "want {:?}, got {errors:?}", rule.text);
    let frames = engine.orders(3);
    assert_eq!(frames.iter().filter(|f| field(f, 35) == Some("F")).count(), 1, "no second cancel");
}

// ── Modify ──

/// Place `first` on `contract`, then the same order id again as
/// `second` on `second_contract`: refused with `rule`, no replace sent.
fn modify_refused(rule: &str, contract: &Contract, first: &Order, second_contract: &Contract, second: &Order) {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, contract, first).unwrap();
    engine.client.place_order(REFUSED, second_contract, second).unwrap();
    let rule = local_rule(rule);
    let errors = engine.errors();
    assert!(errors.iter().any(|(id, code, text)| *id == REFUSED && *code == rule.api_code
        && matches_template(&rule.text, text, false)), "rule {}: want {:?}, got {errors:?}", rule.id, rule.text);
    // A valid order after it: the replace was not sent before it.
    engine.client.place_order(NEXT, &aapl(), &lmt("BUY", 1.0, 100.0)).unwrap();
    let next = NEXT.to_string();
    let frames = engine.orders(2);
    assert!(frames.iter().any(|f| field(f, 6121) == Some(next.as_str())), "the valid order is sent");
    assert!(frames.iter().all(|f| field(f, 35) != Some("G")), "rule {}: no replace sent", rule.id);
}

#[test]
fn modify_to_another_order_type() {
    let stp = Order { aux_price: 90.0, ..order("STP") };
    modify_refused("329-1", &aapl(), &order("LMT"), &aapl(), &stp);
}

#[test]
fn modify_of_the_side() {
    modify_refused("105-1", &aapl(), &order("LMT"), &aapl(), &Order { action: "SELL".into(), ..order("LMT") });
}

#[test]
fn modify_of_the_oca_group() {
    let first = Order { oca_group: "A".into(), ..order("LMT") };
    modify_refused("10326-1", &aapl(), &first, &aapl(), &Order { oca_group: "B".into(), ..first.clone() });
}

#[test]
fn modify_of_the_oca_type() {
    let first = Order { oca_group: "A".into(), oca_type: 3, ..order("LMT") };
    modify_refused("10327-1", &aapl(), &first, &aapl(), &Order { oca_type: 1, ..first.clone() });
}

#[test]
fn combo_modify_with_another_leg() {
    let directed = |legs| Contract { exchange: "ARCA".into(), ..bag(legs) };
    let first = directed(vec![leg(756733, "BUY"), leg(320227571, "SELL")]);
    let second = directed(vec![leg(756733, "BUY"), leg(9999, "SELL")]);
    modify_refused("10059-1", &first, &order("LMT"), &second, &order("LMT"));
}

#[test]
fn stop_order_without_a_stop_price() {
    // The API's unset value.
    refused("321-2", &aapl(), &Order { action: "SELL".into(), aux_price: f64::MAX, ..order("STP") });
}

#[test]
fn stop_order_with_a_stop_price_not_a_number() {
    refused("403-1", &aapl(), &Order { action: "SELL".into(), aux_price: f64::NAN, ..order("STP") });
}

// A MIT or a LIT without its trigger price gets the text of the missing
// stop price (ibx#548, paper 11/10/2026); one that is not a number gets
// the table's text of the trigger price.
#[test]
fn touched_order_without_a_trigger_price() {
    refused("321-2", &aapl(), &Order { aux_price: f64::MAX, ..order("MIT") });
    refused("321-2", &aapl(), &Order { aux_price: f64::MAX, ..order("LIT") });
    refused("361-1", &aapl(), &Order { aux_price: f64::INFINITY, ..order("MIT") });
    refused("361-1", &aapl(), &Order { aux_price: f64::INFINITY, ..order("LIT") });
}

#[test]
fn unknown_action() {
    refused("321-1", &aapl(), &Order { action: "HOLD".into(), ..order("LMT") });
    refused("321-1", &aapl(), &Order { action: String::new(), ..order("LMT") });
}
