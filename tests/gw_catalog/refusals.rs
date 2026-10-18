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
    assert_refused_text(engine, id, rule, None);
}

/// As [`assert_refused`], with `seen` the text the reference 10.51 was seen
/// giving for the rule where it is not the table's.
fn assert_refused_text(engine: &mut Engine, id: i64, rule: &str, seen: Option<&str>) {
    assert_refused_as(engine, id, rule, None, seen);
}

/// As [`assert_refused_text`], with `code` the code the reference 10.51
/// was seen giving where it is not the table's.
fn assert_refused_as(engine: &mut Engine, id: i64, rule: &str, code: Option<i64>, seen: Option<&str>) {
    let rule = local_rule(rule);
    let want = code.unwrap_or(rule.api_code);
    // The engine's refusals come from its thread.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut errors = Vec::new();
    while !errors.iter().any(|e: &(i64, i64, String)| e.0 == id) && std::time::Instant::now() < deadline {
        // The server answers what the engine asks meanwhile (the stock order
        // preset, ibx#728).
        engine.serve();
        errors.extend(engine.errors());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let mine: Vec<&(i64, i64, String)> = errors.iter().filter(|e| e.0 == id).collect();
    assert!(
        mine.iter().any(|(_, code, text)| *code == want && match seen {
            Some(seen) => text == seen,
            None => matches_template(&rule.text, text, false)
                // The table is 10.40's; 10.51 adds the order id to 103 (ibx#625).
                || (rule.api_code == 103 && *text == format!("{}: {}", rule.text, id)),
        }),
        "rule {} (gateway code {}): want {} {:?}, got {:?}", rule.id, rule.code, want, seen.unwrap_or(&rule.text), errors,
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

/// As [`refused`], with the text the reference 10.51 was seen giving.
fn refused_seen(rule: &str, contract: &Contract, order: &Order, seen: &str) {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, contract, order).expect("a refusal is an error callback, not a call error");
    assert_refused_text(&mut engine, REFUSED, rule, Some(seen));
}

/// As [`refused_seen`], with the code the reference 10.51 was seen giving.
fn refused_seen_as(rule: &str, contract: &Contract, order: &Order, code: i64, seen: &str) {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, contract, order).expect("a refusal is an error callback, not a call error");
    assert_refused_as(&mut engine, REFUSED, rule, Some(code), Some(seen));
}

fn order(order_type: &str) -> Order {
    Order { order_type: order_type.into(), ..lmt("BUY", 1.0, 100.0) }
}

// ── The reading of the order: 320 with the rule text, or the date codes ──

// A date that cannot be read is refused with the table's text: 343 on
// 10.51 (ibx#688, paper 15/10/2026), as a zone that is not the
// contract's; 391 on 10.40 (ibx#548, paper 11/10/2026: "not a date").
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
    assert!(errors.iter().any(|(_, code, text)| *code == 343 && matches_template(&rule.text, text, false)), "{errors:?}");
}

// A start time that cannot be read is refused with 337 at once on 10.51
// (ibx#688, paper 15/10/2026; 10.40 sent the order without it, ibx#594);
// a time alone is today's (ibx#598). The table's text is the same as the
// refusal of a zone that is not the contract's.
#[test]
fn good_after_time_not_a_date() {
    let mut engine = Engine::start();
    engine.shared.reference.cache_time_zone_id(aapl().con_id, "US/Eastern");
    engine.client.place_order(REFUSED, &aapl(), &Order { good_after_time: "20261230 10:00:00 Asia/Tokyo".into(), ..order("LMT") }).unwrap();
    assert_refused(&mut engine, REFUSED, "337-1");
    let mut engine = Engine::start();
    engine.client.place_order(6, &aapl(), &Order { good_after_time: "15:30:00".into(), ..order("LMT") }).unwrap();
    assert_eq!(field(&engine.orders(1)[0], 6121), Some("6"), "a time alone is sent");
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { good_after_time: "after lunch".into(), ..order("LMT") }).unwrap();
    assert_refused(&mut engine, REFUSED, "337-1");
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

// Refused at the first checks, 321 (ibx#688; observed on paper,
// 17/10/2026, market closed: a two-leg stock combo with the tag "Foo").
#[test]
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
    // Sent first: the session's first stock order waits for the order
    // preset (ibx#728), and a cancel before it leaves sends nothing.
    assert_eq!(engine.orders(1).len(), 1, "the order is sent");
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
    // The server answered it: placed again, a modify (ibx#688).
    engine.answer_order(REFUSED);
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

// ── The rules new in 10.51 (ibx#688) ──

// (ibx#688) The overnight times in force sent through the classic request
// form: 10052 naming the time in force, nothing sent (ibx#660, paper
// 13/10/2026). The other cases of the rule, a time in force that is not
// known and includeOvernight with another time in force than DAY, are not
// followed yet (ibx#688).
#[test]
fn overnight_time_in_force() {
    refused("10052-1", &aapl(), &Order { tif: "OVERNIGHT".into(), ..order("LMT") });
    refused("10052-1", &aapl(), &Order { tif: "OVERNIGHT + DAY".into(), ..order("LMT") });
}

// (ibx#688) A what-if type on an order that is not a what-if.
#[test]
fn what_if_type_without_what_if() {
    refused("10352-1", &aapl(), &Order { what_if_type: 1, ..order("LMT") });
}

// (ibx#688) A what-if type below 0 or above the highest known one.
#[test]
fn what_if_type_out_of_range() {
    refused("10351-1", &aapl(), &Order { what_if: true, what_if_type: -1, ..order("LMT") });
    refused("10351-1", &aapl(), &Order { what_if: true, what_if_type: 7, ..order("LMT") });
}

// (ibx#688) A what-if type above a preview on an account without the other
// kinds, as the paper account (ibx#608, paper 12/10/2026).
#[test]
fn what_if_type_above_a_preview() {
    refused("10351-2", &aapl(), &Order { what_if: true, what_if_type: 2, ..order("LMT") });
}

fn non_guaranteed() -> Vec<TagValue> {
    vec![TagValue { tag: "NonGuaranteed".into(), value: "1".into() }]
}

// (ibx#688) A non-guaranteed combo that is not routed to SMART.
#[test]
fn non_guaranteed_combo_not_smart() {
    let directed = Contract { exchange: "ARCA".into(), ..bag(vec![leg(756733, "BUY"), leg(320227571, "SELL")]) };
    refused("10368-1", &directed, &Order { smart_combo_routing_params: non_guaranteed(), ..order("LMT") });
}

// (ibx#688) A combo of three legs with a limit order type and no limit
// price (ibx#660, observed 13/10/2026).
#[test]
fn combo_of_three_legs_without_a_limit_price() {
    let directed = Contract {
        exchange: "ARCA".into(),
        ..bag(vec![leg(756733, "BUY"), leg(320227571, "SELL"), leg(9579970, "BUY")])
    };
    refused("10369-1", &directed, &Order { lmt_price: f64::MAX, ..order("LMT") });
}

// (ibx#688) Post-only on an order type that cannot rest as post-only
// (ibx#607, paper 12/10/2026).
#[test]
fn post_only_on_a_market_order() {
    refused("10051-1", &aapl(), &Order { post_only: true, ..order("MKT") });
}

// (ibx#688) Post-only routed to the broker's own alternative trading
// system.
#[test]
fn post_only_on_the_own_trading_system() {
    let own = Contract { exchange: "IBKRATS".into(), ..aapl() };
    refused("464-1", &own, &Order { post_only: true, ..order("LMT") });
}

// (ibx#688) Post-only on the broker's own accumulate/distribute algo
// (ibx#623).
#[test]
fn post_only_on_the_own_algo() {
    refused("442-1", &aapl(), &Order { post_only: true, algo_strategy: "AD".into(), ..order("LMT") });
}

/// The text the reference 10.51 gives for the open flags with the time in
/// force OVERNIGHT + DAY: the table's, with the time in force after it
/// (paper 12/10/2026).
const OVERNIGHT_DAY_SEEN: &str = "Invalid time in force:OVERNIGHT + DAY";

// (ibx#688) Post-only with the time in force OVERNIGHT + DAY.
#[test]
fn post_only_overnight_and_day() {
    let order = Order { post_only: true, tif: "OVERNIGHT + DAY".into(), ..order("LMT") };
    refused_seen("10052-6", &aapl(), &order, OVERNIGHT_DAY_SEEN);
}

// (ibx#688) Pre-open or opening auction flags on an overnight venue.
#[test]
fn open_flags_on_an_overnight_venue() {
    let on = |exchange: &str| Contract { exchange: exchange.into(), ..aapl() };
    refused("464-2", &on("OVERNIGHT"), &Order { allow_pre_open: true, ..order("LMT") });
    refused("464-2", &on("IBEOS"), &Order { ignore_open_auction: true, ..order("LMT") });
}

// (ibx#688) Pre-open or opening auction flags with the time in force
// OVERNIGHT + DAY.
#[test]
fn open_flags_overnight_and_day() {
    let tif = "OVERNIGHT + DAY".to_string();
    refused_seen("10052-7", &aapl(), &Order { allow_pre_open: true, tif: tif.clone(), ..order("LMT") }, OVERNIGHT_DAY_SEEN);
    refused_seen("10052-7", &aapl(), &Order { ignore_open_auction: true, tif, ..order("LMT") }, OVERNIGHT_DAY_SEEN);
}

// (ibx#688) Skipping the opening auction with the scale auto-reset.
#[test]
fn ignore_open_auction_with_scale_auto_reset() {
    refused("10340-3", &aapl(), &Order { ignore_open_auction: true, scale_auto_reset: true, ..order("LMT") });
}

/// An order asking for attached orders: `sl` and `pt`, an id (or the API's
/// unset value) and a type, for the stop loss and the profit taker.
fn attached(sl: (i32, &str), pt: (i32, &str)) -> Order {
    Order {
        sl_order_id: sl.0, sl_order_type: sl.1.into(), pt_order_id: pt.0, pt_order_type: pt.1.into(),
        ..order("LMT")
    }
}

const UNSET: i32 = i32::MAX;

/// The reading refusals of the attached orders, as the reference 10.51
/// gives them (ibx#609; paper 15 and 16/10/2026): "Error reading request."
/// then a space before the cause, where the table has a colon.
fn read_seen(rule: &str) -> String {
    let text = local_rule(rule).text;
    let cause = text.strip_prefix("Error reading request:").expect("a reading refusal");
    format!("Error reading request. {cause}")
}

// (ibx#688) Attached orders on a session whose logon forbids attaching
// them in one request.
#[test]
fn attached_orders_not_in_one_request() {
    let mut engine = Engine::start();
    engine.shared.reference.set_logon_features("NOAPISLPTSGL");
    engine.client.place_order(REFUSED, &aapl(), &attached((8, "PRESET"), (9, "PRESET"))).unwrap();
    assert_refused_text(&mut engine, REFUSED, "10354-1", Some(&read_seen("10354-1")));
}

// (ibx#688) A stop-loss leg with an id and no PRESET type.
#[test]
fn attached_stop_loss_without_its_type() {
    refused_seen("10356-1", &aapl(), &attached((8, ""), (UNSET, "")), &read_seen("10356-1"));
}

// (ibx#688) A profit-taker leg with the PRESET type and no id.
#[test]
fn attached_profit_taker_without_its_id() {
    refused_seen("10356-2", &aapl(), &attached((UNSET, ""), (UNSET, "PRESET")), &read_seen("10356-2"));
}

// (ibx#688) An attached order id that is not a valid id.
#[test]
fn attached_order_id_zero() {
    refused("10149-2", &aapl(), &attached((UNSET, ""), (0, "PRESET")));
}

// (ibx#688) An attached order id below the highest used.
#[test]
fn attached_order_id_below_the_highest_used() {
    let mut engine = Engine::start();
    engine.client.place_order(50, &aapl(), &order("LMT")).unwrap();
    assert_eq!(engine.orders(1).len(), 1, "the first order is sent");
    engine.client.place_order(60, &aapl(), &attached((UNSET, ""), (40, "PRESET"))).unwrap();
    assert_refused(&mut engine, 60, "103-2");
}

// (ibx#688) An attached order under the parent's own id, or both legs
// under one id (observed 16/10/2026).
#[test]
fn attached_order_ids_not_all_different() {
    refused("103-3", &aapl(), &attached((UNSET, ""), (REFUSED as i32, "PRESET")));
    refused("103-3", &aapl(), &attached((8, "PRESET"), (8, "PRESET")));
}

// (ibx#688) Attached orders asked from an order preset that does not
// define them: the preset is asked first, then the profit taker is named
// first, the stop loss alone names itself (ibx#609, paper 16/10/2026);
// the order is dropped, nothing sent.
#[test]
fn attached_from_a_preset_without_them() {
    for (rule, order) in [
        ("10355-1", attached((8, "PRESET"), (9, "PRESET"))),
        ("10355-2", attached((8, "PRESET"), (UNSET, ""))),
    ] {
        let mut engine = Engine::start();
        engine.client.place_order(REFUSED, &aapl(), &order).unwrap();
        engine.answer_preset("STK", "DAY");
        assert_refused(&mut engine, REFUSED, rule);
    }
}

// (ibx#688) An order placed with no time in force takes the one of the
// order preset of its type, with the notice, and is sent with it (ibx#660,
// paper 13/10/2026: the notice ends with a period).
#[test]
fn time_in_force_from_the_order_preset() {
    ibx::client_core::set_session_level_for_test(Some(214)); // a capture at level 214 (ibx#602)
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { tif: String::new(), ..order("LMT") }).unwrap();
    engine.answer_preset("STK", "GTC");
    let frames = engine.orders(1);
    let id = REFUSED.to_string();
    let sent: Vec<&ibx::test_support::Fields> = frames.iter().filter(|f| field(f, 6121) == Some(id.as_str())).collect();
    assert_eq!(sent.len(), 1, "the order is sent: {frames:?}");
    assert_eq!(field(sent[0], 59), Some("1"), "with the preset's time in force");
    let rule = local_rule("10349-1");
    let errors = engine.errors();
    assert!(errors.iter().any(|(oid, code, text)| *oid == REFUSED && *code == rule.api_code
        && *text == "Order TIF was set to GTC based on order preset."), "{errors:?}");
}

// (ibx#688) Route marketable orders to the best bid or offer only, with
// the bypass of its warning off (the default): the warning, then the
// order is discarded, nothing sent (ibx#608, paper 12/10/2026).
#[test]
fn route_marketable_to_bbo_without_the_bypass() {
    refused("10353-1", &aapl(), &Order { route_marketable_to_bbo: true, ..order("LMT") });
}

// (ibx#688) A selling beta hedge without its max size, under a parent
// held with transmit off (ibx#614, paper 15/10/2026): refused, nothing
// sent, the parent stays held.
#[test]
fn selling_beta_hedge_without_a_max_size() {
    let mut engine = Engine::start();
    engine.client.place_order(6, &aapl(), &Order { transmit: false, ..order("LMT") }).unwrap();
    let hedge = Order {
        action: "SELL".into(), parent_id: 6, hedge_type: "B".into(), hedge_param: "1".into(), lmt_price: f64::MAX,
        total_quantity: 0.0, ..order("MKT")
    };
    engine.wait_held_known(6);
    engine.client.place_order(REFUSED, &aapl(), &hedge).unwrap();
    assert_refused(&mut engine, REFUSED, "10364-1");
    assert!(engine.orders(0).iter().all(|f| field(f, 6121) != Some("6")), "the parent stays held");
}

// (ibx#602) A selling beta hedge whose max size is below the hedge's size
// (the reference 10.51, paper, regular hours 16/10/2026: a parent of 10, a
// beta of 1 and a max size of 2): the hedge-size precaution's warning on
// the parent, which is discarded, and nothing sent for either order.
#[test]
fn beta_hedge_with_a_max_size_below_the_hedge() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { transmit: false, total_quantity: 10.0, ..order("LMT") }).unwrap();
    let hedge = Order {
        action: "SELL".into(), parent_id: REFUSED, hedge_type: "B".into(), hedge_param: "1".into(), hedge_max_size: 2,
        lmt_price: f64::MAX, total_quantity: 0.0, ..order("MKT")
    };
    engine.wait_held_known(REFUSED);
    engine.client.place_order(REFUSED + 1, &aapl(), &hedge).unwrap();
    assert_refused_text(&mut engine, REFUSED, "2185-1",
        Some("Max Hedging Size Warning
Restriction is specified in Precautionary Settings of Global Configuration/Presets."));
    let hedge_id = (REFUSED + 1).to_string();
    assert!(engine.orders(0).iter().all(|f| field(f, 6121) != Some(hedge_id.as_str())), "the hedge is not sent");
}

// ── The rules of the 10.51 table taken over from 10.40 (ibx#688) ──

// (ibx#688) A short sale on an institutional session without its short
// sale slot.
#[test]
fn short_sale_without_a_slot() {
    short_sale("347-1", 0, "", -1);
}

/// The text the reference 10.51 gives for an order whose contract has no
/// exchange: the table's, with a final period (paper 15/10/2026, cases AA
/// and Q).
const NO_EXCHANGE_SEEN: &str = "Error validating request.-'PlaceOrder(3)' : cause - Missing order exchange.";

// (ibx#688) An order whose contract has no exchange, the official API's
// default.
#[test]
fn order_without_an_exchange() {
    let no_exchange = Contract { exchange: String::new(), ..aapl() };
    refused_seen("452-1", &no_exchange, &order("LMT"), NO_EXCHANGE_SEEN);
}

// (ibx#688) A volatility order whose volatility type is not 1 or 2, unset
// or out of range (ibx#673; observed 16/10/2026, regular hours, paper, an
// option without a volatility type).
#[test]
fn volatility_order_without_its_type() {
    refused("367-1", &aapl(), &Order { volatility: 30.0, ..order("VOL") });
    refused("367-1", &aapl(), &Order { volatility: 30.0, volatility_type: 3, ..order("VOL") });
}

// (ibx#688) An active time of the broker's own accumulate/distribute algo
// that is not in the hh:mm:ss form (ibx#623, paper 15/10/2026).
#[test]
fn own_algo_active_time_not_a_time() {
    refused("10315-2", &aapl(), &Order {
        algo_strategy: "AD".into(),
        algo_params: vec![TagValue { tag: "activeTimeStart".into(), value: "25:00:00".into() }],
        ..order("LMT")
    });
}

/// A hedge child of `parent_id`: its kind and parameter, with no size and
/// no price of its own (the gateway sizes it).
fn hedge(parent_id: i64, kind: &str, param: &str) -> Order {
    Order {
        action: "SELL".into(), parent_id, hedge_type: kind.into(), hedge_param: param.into(), lmt_price: f64::MAX,
        total_quantity: 0.0, ..order("MKT")
    }
}

/// `child` on `contract` under an AAPL parent 6 held with transmit off:
/// refused with `rule` (the text `seen` where given), nothing sent, the
/// parent stays held.
fn hedge_refused(rule: &str, contract: &Contract, child: &Order, seen: Option<&str>) {
    let mut engine = Engine::start();
    engine.client.place_order(6, &aapl(), &Order { transmit: false, ..order("LMT") }).unwrap();
    // The hedge's own rules: its parent is known first (ibx#718).
    engine.wait_held_known(6);
    engine.client.place_order(REFUSED, contract, child).unwrap();
    assert_refused_text(&mut engine, REFUSED, rule, seen);
    assert!(engine.orders(0).iter().all(|f| field(f, 6121) != Some("6")), "the parent stays held");
}

// The reading refusals of a hedge are given as the reference 10.51 gives
// them (ibx#614; paper 16/10/2026): "Error reading request." then a space
// before the cause, where the table has a colon.

// (ibx#688) A hedge kind that is not one of the known letters (observed
// 16/10/2026).
#[test]
fn hedge_of_an_unknown_kind() {
    hedge_refused("10007-1", &aapl(), &hedge(6, "X", ""), Some(&read_seen("10007-1")));
}

// (ibx#688) A hedge whose parent was not placed before it: the text names
// the client id (0 on the reference), the order, the parent and the kind
// (observed 16/10/2026).
#[test]
fn hedge_without_its_parent() {
    refused_seen("10006-1", &aapl(), &hedge(60, "B", "1"),
        "Error reading request. Missing parent order: server: 0 order: 7 parent: 60 hedge:[B]");
}

// (ibx#688) A delta hedge under a parent that is no option: the text
// ends with the client id (0), an order id of 0, the parent and the kind
// (observed 16/10/2026).
#[test]
fn delta_hedge_not_on_an_option() {
    hedge_refused("10010-1", &aapl(), &hedge(6, "D", ""), Some("Error reading request. Invalid delta hedge order. The parent order has to be option order and hedge order can only be Stock, CFD or (in case of option on Index) Futures order. server: 0 order: 0 parent: 6 hedge:[D]"));
}

// (ibx#688) A pair hedge without its ratio (observed 16/10/2026).
#[test]
fn pair_hedge_without_a_ratio() {
    hedge_refused("10009-1", &aapl(), &hedge(6, "P", ""), Some(&read_seen("10009-1")));
}

// (ibx#688) A beta hedge on a contract that is no stock; not observed
// yet, the text in the form of the other hedge rules.
#[test]
fn beta_hedge_not_on_a_stock() {
    let cfd = Contract { sec_type: "CFD".into(), ..aapl() };
    hedge_refused("10033-1", &cfd, &hedge(6, "B", "1"), Some(&read_seen("10033-1")));
}

// (ibx#688) A beta hedge whose beta is not a number (observed
// 16/10/2026).
#[test]
fn beta_hedge_with_a_beta_not_a_number() {
    hedge_refused("10008-1", &aapl(), &hedge(6, "B", "abc"), Some(&read_seen("10008-1")));
}

// (ibx#688) A hedge with a limit price of its own; not observed yet.
#[test]
fn hedge_with_a_limit_price() {
    hedge_refused("10031-1", &aapl(), &Order { lmt_price: 100.0, ..hedge(6, "P", "2") }, None);
}

// (ibx#688) A hedge with a size of its own (ibx#614, observed 16/10/2026).
#[test]
fn hedge_with_a_size() {
    hedge_refused("10032-1", &aapl(), &Order { total_quantity: 5.0, ..hedge(6, "P", "2") }, None);
}

// ── Observed on paper, 17/10/2026, market closed (ibx#688): one far AAPL
// order each, a client at level 214, the refusal by an error callback with
// the order id, nothing sent; ibx used to fail in the call. ──

const CAUSE: &str = "Error validating request.-'PlaceOrder(3)' : cause - ";

// A time in force that is not known, and GTC on an order for the
// overnight session: 10052 with the time in force.
#[test]
fn time_in_force_not_known_or_not_day_overnight() {
    refused_seen("10052-1", &aapl(), &Order { tif: "XYZ".into(), ..order("LMT") }, "Invalid time in force:XYZ");
    refused_seen("10052-1", &aapl(), &Order { tif: "GTC".into(), include_overnight: true, ..order("LMT") },
        "Invalid time in force:GTC");
}

// A future's contract month that cannot be read: 10372 with the table's
// text, before any lookup of the contract.
#[test]
fn contract_month_not_readable() {
    let mes = Contract {
        symbol: "MES".into(), sec_type: "FUT".into(), exchange: "CME".into(), currency: "USD".into(),
        last_trade_date_or_contract_month: "2026AB".into(), ..Default::default()
    };
    refused("10372-1", &mes, &Order { lmt_price: 1000.0, ..order("LMT") });
}

// A name that is no order type: 321 "Invalid order type was entered",
// where the table gives 10051.
#[test]
fn order_type_not_known() {
    refused_seen_as("10051-2", &aapl(), &order("XYZ"), 321, &format!("{CAUSE}Invalid order type was entered"));
}

// SSHORTX: the table's text, with no space at its end.
#[test]
fn sshortx_not_supported() {
    refused_seen("498-1", &aapl(), &Order { action: "SSHORTX".into(), ..order("LMT") },
        &format!("{CAUSE}SSHORTX orders are not supported"));
}

// A TRAIL without its trailing amount: the table's text, the order type
// named "TRAIL Order".
#[test]
fn trail_without_a_trailing_amount() {
    refused_seen("10028-1", &aapl(), &Order { lmt_price: 0.0, aux_price: f64::MAX, trailing_percent: f64::MAX, ..order("TRAIL") },
        &format!("{CAUSE}You must enter a Trailing Amount for TRAIL Order."));
}

// A TRAIL LIMIT without its trailing amount nor its trailing stop price:
// the stop price is asked first.
#[test]
fn trail_limit_without_a_trailing_amount() {
    refused_seen("321-2", &aapl(), &Order {
        lmt_price: 0.0, lmt_price_offset: 0.1, aux_price: f64::MAX, trailing_percent: f64::MAX, trail_stop_price: f64::MAX,
        ..order("TRAIL LIMIT")
    }, &format!("{CAUSE}Please enter a trailing stop price"));
}

// ── Observed on paper, 17/10/2026, market closed, second round (ibx#688):
// one far AAPL order each, a client at level 214, nothing sent; ibx used to
// send these orders, or to give another code. ──

/// The date and time text of the reference with its field's label.
fn date_time_text(label: &str) -> String {
    local_rule("343-1").text.replacen("%s", label, 1)
}

// A delta-neutral contract on an order that is no combo: 320 at once,
// "Error reading request." then a space, with a final period.
#[test]
fn delta_neutral_contract_without_combo_legs() {
    let contract = Contract {
        delta_neutral_contract: Some(ibx::api::types::DeltaNeutralContract { con_id: 265598, delta: 0.5, price: 100.0 }),
        ..aapl()
    };
    refused_seen("429-1", &contract, &order("LMT"),
        "Error reading request. Delta neutral orders are only supported for combos (BAG security type).");
}

// A manual order time that cannot be read: 10300 with the date and time
// text, labelled "Manual Order Time".
#[test]
fn manual_order_time_not_readable() {
    refused_seen("10300-1", &aapl(), &Order { manual_order_time: "garbage".into(), ..order("LMT") },
        &date_time_text("Manual Order Time"));
}

// An active start time not in the hh:mm:ss form: 10315 labelled "Active
// Start Time"; the stop time takes its own label (not observed yet).
#[test]
fn active_time_not_a_time() {
    refused_seen("10315-1", &aapl(), &Order { active_start_time: "garbage".into(), ..order("LMT") },
        "Active Start Time: The time entered is invalid. The correct format is hh:mm:ss. E.g.: 15:00:00 in UTC. No date should be specified, current date is assumed.");
    refused_seen("10315-1", &aapl(), &Order { active_stop_time: "25:00:00".into(), ..order("LMT") },
        "Active Stop Time: The time entered is invalid. The correct format is hh:mm:ss. E.g.: 15:00:00 in UTC. No date should be specified, current date is assumed.");
}

// A time condition without its time: 10314, as one that cannot be read.
#[test]
fn time_condition_without_a_time() {
    refused("10314-1", &aapl(), &Order {
        conditions: vec![OrderCondition::Time { time: String::new(), is_more: true }],
        ..order("LMT")
    });
}

// A duration with a time in force other than GTD, minutes included.
#[test]
fn duration_without_gtd() {
    refused("10272-1", &aapl(), &Order { duration: 100, ..order("LMT") });
    refused("10272-1", &aapl(), &Order { duration: 5, tif: "Minutes".into(), ..order("LMT") });
}

// GTD with an end time and a duration.
#[test]
fn gtd_with_an_end_time_and_a_duration() {
    refused("10273-1", &aapl(), &Order {
        tif: "GTD".into(), good_till_date: "20261120 15:00:00 US/Eastern".into(), duration: 100, ..order("LMT")
    });
}

// GTD with neither an end time nor a duration: 391 with the date and
// time text labelled "End Time".
#[test]
fn gtd_without_an_end_time_or_a_duration() {
    refused_seen("391-1", &aapl(), &Order { tif: "GTD".into(), ..order("LMT") }, &date_time_text("End Time"));
}

// A size of 0: the table's text, ending with a colon.
#[test]
fn size_zero() {
    refused_seen("160-1", &aapl(), &Order { total_quantity: 0.0, ..order("LMT") },
        &format!("{CAUSE}The size value cannot be zero:"));
}

// Pegged to benchmark without its reference contract.
#[test]
fn pegged_to_benchmark_without_its_reference() {
    refused("10075-1", &aapl(), &Order { reference_contract_id: 0, ..order("PEG BENCH") });
}

// A child under a parent this client does not have: 135 with the parent's
// id after the text.
#[test]
fn child_of_an_unknown_parent() {
    refused_seen("135-1", &aapl(), &Order { parent_id: 1, ..order("LMT") }, "Can't find order with id = 1");
}

// A scale order whose first component is larger than the order.
#[test]
fn scale_first_component_above_the_order() {
    refused("417-1", &aapl(), &Order { scale_init_level_size: 10, scale_price_increment: 0.1, ..order("LMT") });
}

// A forex hedge on a contract that is no currency pair: its own code,
// the parent stays held.
#[test]
fn fx_hedge_not_on_a_currency_pair() {
    hedge_refused("10063-1", &aapl(), &hedge(6, "F", ""), None);
}

/// `engine` kept the refused order `id` for its cancel: its cancel ends
/// it as an order never sent (202 "Order was discarded", then 161).
fn assert_kept(engine: &mut Engine, id: i64) {
    engine.client.cancel_order(id, "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut errors = Vec::new();
    while !errors.iter().any(|e: &(i64, i64, String)| e.0 == id && e.1 == 161) && std::time::Instant::now() < deadline {
        errors.extend(engine.errors());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let mine: Vec<(i64, String)> = errors.iter().filter(|e| e.0 == id).map(|e| (e.1, e.2.clone())).collect();
    assert_eq!(mine.first(), Some(&(202, "Order Canceled - reason:Order was discarded.".to_string())), "{errors:?}");
    assert!(mine.iter().any(|(code, _)| *code == 161), "{errors:?}");
}

/// Place `order` on AAPL: refused with `rule` (the text `seen` where
/// given), nothing sent, kept for its cancel.
fn kept_refused(rule: &str, order: &Order, seen: Option<&str>) {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), order).unwrap();
    assert_refused_text(&mut engine, REFUSED, rule, seen);
    assert_kept(&mut engine, REFUSED);
}

fn price_condition() -> OrderCondition {
    OrderCondition::Price {
        con_id: 265598, exchange: "SMART".into(), price: 1000 * ibx::types::PRICE_SCALE, is_more: true, trigger_method: 0,
    }
}

/// The text of 148 as the reference 10.51 gives it: the table's two
/// sentences joined by one space.
const CONDITIONS_SEEN: &str = "Conditional submission of orders is supported for Limit, Market, MidPrice, Relative and Snap order types only. Conditional cancellation of orders is supported for Limit, MidPrice, IBAlgo - Limit order types only.";

// Conditions on a stop order, and conditions that cancel a relative
// order: 148, the order kept for its cancel.
#[test]
fn conditions_on_an_order_type_without_them() {
    kept_refused("148-1", &Order { aux_price: 500.0, conditions: vec![price_condition()], ..order("STP") }, Some(CONDITIONS_SEEN));
    kept_refused("148-2", &Order {
        aux_price: 0.01, lmt_price: 0.0, conditions: vec![price_condition()], conditions_cancel_order: true, ..order("REL")
    }, Some(CONDITIONS_SEEN));
}

// An OCA group name longer than 64 characters: 401 "OCA Group", the
// order kept for its cancel (the server closed the session when ibx sent
// one).
#[test]
fn oca_group_name_too_long() {
    kept_refused("401-1", &Order { oca_group: "O".repeat(70), ..order("LMT") }, Some("OCA Group"));
}

// A trailing stop under a held stop order: 328 on the child, kept for its
// cancel; the parent stays held.
#[test]
fn trailing_stop_under_a_stop_parent() {
    let mut engine = Engine::start();
    engine.client.place_order(6, &aapl(), &Order { aux_price: 500.0, transmit: false, ..order("STP") }).unwrap();
    let child = Order { action: "SELL".into(), aux_price: 1.0, lmt_price: 0.0, parent_id: 6, ..order("TRAIL") };
    engine.client.place_order(REFUSED, &aapl(), &child).unwrap();
    assert_refused(&mut engine, REFUSED, "328-1");
    assert!(engine.orders(0).iter().all(|f| field(f, 6121) != Some("6")), "the parent stays held");
    assert_kept(&mut engine, REFUSED);
}

// Opting out of smart routing: the warning 2181 with "Warning: " before
// the table's text, and the order is sent.
#[test]
fn opt_out_of_smart_routing_is_a_warning() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { opt_out_smart_routing: true, ..order("LMT") }).unwrap();
    let frames = engine.orders(1);
    assert!(frames.iter().any(|f| field(f, 6121) == Some("7")), "the order is sent: {frames:?}");
    let rule = local_rule("2181-1");
    let errors = engine.errors();
    assert!(errors.iter().any(|(id, code, text)| *id == REFUSED && *code == rule.api_code
        && *text == format!("Warning: {}", rule.text)), "{errors:?}");
}

// A modify that adds a duration to a GTD order with an end time.
#[test]
fn modify_adding_a_duration_to_gtd() {
    let first = Order { tif: "GTD".into(), good_till_date: "20261120 15:00:00 US/Eastern".into(), ..order("LMT") };
    modify_refused("10273-2", &aapl(), &first, &aapl(), &Order { duration: 100, ..first.clone() });
}

// The same order id placed again before the server answered the order:
// 103, the order stays as placed.
#[test]
fn order_placed_again_before_its_answer() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &order("LMT")).unwrap();
    engine.client.place_order(REFUSED, &aapl(), &Order { lmt_price: 101.0, ..order("LMT") }).unwrap();
    let rule = local_rule("103-1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut errors = Vec::new();
    while errors.is_empty() && std::time::Instant::now() < deadline {
        engine.serve();
        errors.extend(engine.errors());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(errors.iter().any(|(id, code, text)| *id == REFUSED && *code == rule.api_code
        && *text == format!("{}: {}", rule.text, REFUSED)), "{errors:?}");
    let frames = engine.orders(1);
    assert!(frames.iter().all(|f| field(f, 35) != Some("G")), "no replace sent: {frames:?}");
}

// ── Written as the reference writes them (ibx#688, paper 17/10/2026) ──

// A GTD order with a duration: the duration in its attribute after the
// origin, no end time.
#[test]
fn gtd_with_a_duration_is_written_with_it() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { tif: "GTD".into(), duration: 3600, ..order("LMT") }).unwrap();
    let frames = engine.orders(1);
    let sent = frames.iter().find(|f| field(f, 6121) == Some("7")).expect("the order is sent");
    assert_eq!(field(sent, 8402), Some("3600"), "{sent:?}");
    assert_eq!((field(sent, 432), field(sent, 126)), (None, None));
    let tags: Vec<u32> = sent.iter().map(|(t, _)| *t).collect();
    let at = |tag: u32| tags.iter().position(|t| *t == tag).unwrap();
    assert!(at(6122) < at(8402), "after the origin: {tags:?}");
}

// A limit order at a price of 0 goes without its price.
#[test]
fn limit_order_at_zero_is_written_without_its_price() {
    let mut engine = Engine::start();
    engine.client.place_order(REFUSED, &aapl(), &Order { lmt_price: 0.0, ..order("LMT") }).unwrap();
    let frames = engine.orders(1);
    let sent = frames.iter().find(|f| field(f, 6121) == Some("7")).expect("the order is sent");
    assert_eq!(field(sent, 44), None, "{sent:?}");
}
