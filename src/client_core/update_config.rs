//! The checks the reference makes on an update of the configuration before
//! it changes anything (session level 221, ibx#612), and the answer that
//! refuses it: "error" for a read-only field or an invalid value, "pending"
//! for a message whose disabling needs its warning accepted.
//!
//! Observed on the reference 10.51 on 17/10/2026 (paper, market closed, the
//! official client at level 225; 37 cases, two fresh processes). What a
//! request that passes the checks is answered, and the configuration the
//! rules read (its messages, the settings the session cannot change, the
//! routing strategies), are the decision asked on ibx#610: they come in
//! `ConfigRules`.

use crate::api::config::{UpdateConfigRequest, UpdateConfigResponse, UpdateConfigWarning};

pub const STATUS_ERROR: &str = "error";
pub const STATUS_PENDING: &str = "pending";
pub const READ_ONLY_FAILED: &str =
    "Validating configuration update request has failed. This setting is read-only and cannot be modified via API";
pub const VALIDATION_FAILED: &str = "Validating configuration update request has failed";
pub const CONFIRMATION_NEEDED: &str = "Configuration update requires confirmation. Please review the warnings and \
                                       re-submit your request with acceptedWarnings populated.";

/// A message of the configuration as the checks see it.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRule {
    pub id: i32,
    /// The warning (title, text) to accept before the message is disabled,
    /// for the order confirmations; `None` for the others.
    pub warning: Option<(String, String)>,
}

/// What the checks read from the configuration and the session.
#[derive(Clone, Debug, Default)]
pub struct ConfigRules {
    /// The messages an update can name (those the configuration lists).
    pub messages: Vec<MessageRule>,
    /// The settings this session cannot change, by the name the errors give
    /// them (`letApiAccountRequestsSwitchSubscription` on the paper session
    /// of 17/10/2026).
    pub settings_off: Vec<&'static str>,
    /// The routing strategies `defaultAlgorithm` can name.
    pub algorithms: Vec<String>,
    /// `preOpenReroute` can be changed.
    pub pre_open_reroute: bool,
    /// `doNotRouteToDarkPools` can be changed.
    pub dark_pools: bool,
}

const SETTINGS: &str = "configuration.api.settings.";

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|a| a.eq_ignore_ascii_case(value))
}

/// The answer that refuses `request`, or `None` when it passes the checks.
/// The reference makes three lists (read-only fields, invalid values,
/// warnings to accept) and answers with the first that is not empty;
/// nothing is changed then.
pub fn update_config_check(request: &UpdateConfigRequest, rules: &ConfigRules) -> Option<UpdateConfigResponse> {
    let mut read_only: Vec<String> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    let mut warnings: Vec<UpdateConfigWarning> = Vec::new();

    if let Some(l) = &request.lock_and_exit {
        for (set, name) in [
            (l.auto_logoff_time.is_some(), "autoLogoffTime"),
            (l.auto_logoff_period.is_some(), "autoLogoffPeriod"),
            (l.auto_logoff_type.is_some(), "autoLogoffType"),
        ] {
            if set {
                read_only.push(format!("configuration.lockAndExit.{name}"));
            }
        }
    }
    let api = request.api.as_ref();
    let settings = api.and_then(|a| a.settings.as_ref());
    if let Some(s) = settings {
        // A setting given a value outside its range or list (texts compared
        // without case: "ASCII7" is taken), or one this session cannot
        // change.
        let off = |name: &str| rules.settings_off.contains(&name);
        let checks: [(bool, &str); 18] = [
            (s.socket_port.is_some_and(|p| !(1024..=65535).contains(&p)), "socketPort"),
            (s.encode_api_messages.as_deref().is_some_and(|v| !one_of(v, &["ascii7", "osCodePage"])), "encodeApiMessages"),
            (s.logging_level.as_deref().is_some_and(|v| !one_of(v, &["system", "error", "warning", "info", "detail"])), "loggingLevel"),
            (s.send_instrument_timezone.as_deref().is_some_and(|v| !one_of(v, &["instrument", "operator", "utc"])), "sendInstrumentTimezone"),
            (s.historical_data_max_size.is_some_and(|v| ![1, 4, 16].contains(&v)), "historicalDataMaxSize"),
            (s.read_only_api.is_some() && off("readOnlyApi"), "readOnlyApi"),
            (s.option_exercise_request_type.as_deref()
                .is_some_and(|v| off("optionExerciseRequestType") || !one_of(v, &["editable", "final"])), "optionExerciseRequestType"),
            (s.maintain_and_resubmit_orders_on_reconnect.is_some() && off("maintainAndResubmitOrdersOnReconnect"), "maintainAndResubmitOrdersOnReconnect"),
            (s.send_forex_data_in_compatibility_mode.is_some() && off("sendForexDataInCompatibilityMode"), "sendForexDataInCompatibilityMode"),
            (s.compatibility_mode_nasdaq_stocks.is_some() && off("compatibilityModeNasdaqStocks"), "compatibilityModeNasdaqStocks"),
            (s.allow_forex_trading_in_1_10_pips.is_some() && off("allowForexTradingIn110Pips"), "allowForexTradingIn110Pips"),
            (s.show_forex_data_in_1_10_pips.is_some() && off("showForexDataIn110Pips"), "showForexDataIn110Pips"),
            (s.use_account_groups_with_allocation_methods.is_some() && off("useAccountGroupsWithAllocationMethods"), "useAccountGroupsWithAllocationMethods"),
            (s.let_api_account_requests_switch_subscription.is_some() && off("letApiAccountRequestsSwitchSubscription"), "letApiAccountRequestsSwitchSubscription"),
            (s.use_negative_auto_range.is_some() && off("useNegativeAutoRange"), "useNegativeAutoRange"),
            (s.send_status_updates_for_volatility_orders.is_some() && off("sendStatusUpdatesForVolatilityOrders"), "sendStatusUpdatesForVolatilityOrders"),
            (s.download_open_orders_on_connection.is_some() && off("downloadOpenOrdersOnConnection"), "downloadOpenOrdersOnConnection"),
            (s.master_client_id.is_some_and(|v| v < 0 || off("masterClientId")), "masterClientId"),
        ];
        invalid.extend(checks.into_iter().filter(|(bad, _)| *bad).map(|(_, name)| format!("{SETTINGS}{name}")));
        if s.allow_localhost_only.is_some() {
            read_only.push(format!("{SETTINGS}allowLocalhostOnly"));
        }
    }
    if api.is_some_and(|a| a.precautions.is_some()) {
        read_only.push("configuration.api.precautions".to_string());
    }
    if let Some(r) = request.orders.as_ref().and_then(|o| o.smart_routing.as_ref()) {
        if r.pre_open_reroute.is_some() && !rules.pre_open_reroute {
            invalid.push("configuration.orders.smartRouting.preOpenReroute".to_string());
        }
        if r.default_algorithm.as_ref().is_some_and(|a| !rules.algorithms.contains(a)) {
            invalid.push("configuration.orders.smartRouting.defaultAlgorithm".to_string());
        }
        if r.do_not_route_to_dark_pools.is_some() && !rules.dark_pools {
            invalid.push("configuration.orders.smartRouting.doNotRouteToDarkPools".to_string());
        }
    }
    for m in &request.messages {
        let Some(id) = m.id else {
            invalid.push("configuration.messages: Message configuration missing required 'id' field".to_string());
            continue;
        };
        // One read-only field per message: the first of these.
        if let Some(name) = [(m.title.is_some(), "title"), (m.message.is_some(), "message"), (m.default_action.is_some(), "defaultAction")]
            .into_iter().find_map(|(set, name)| set.then_some(name))
        {
            read_only.push(format!("configuration.messages.{name}"));
            continue;
        }
        let Some(rule) = rules.messages.iter().find(|r| r.id == id) else {
            invalid.push(format!("configuration.messages: Message with id {id} not found"));
            continue;
        };
        if let (Some(false), Some((title, text))) = (m.enabled, &rule.warning) {
            if !request.accepted_warnings.iter().any(|w| w.message_id == Some(id)) {
                warnings.push(UpdateConfigWarning { message_id: Some(id), title: Some(title.clone()), message: Some(text.clone()) });
            }
        }
    }

    let answer = |status: &str, message: &str| UpdateConfigResponse {
        // Without a request id the answer has the largest one.
        req_id: Some(request.req_id.unwrap_or(i32::MAX)),
        status: Some(status.to_string()),
        message: Some(message.to_string()),
        ..Default::default()
    };
    if !read_only.is_empty() {
        Some(UpdateConfigResponse { errors: read_only, ..answer(STATUS_ERROR, READ_ONLY_FAILED) })
    } else if !invalid.is_empty() {
        Some(UpdateConfigResponse { errors: invalid, ..answer(STATUS_ERROR, VALIDATION_FAILED) })
    } else if !warnings.is_empty() {
        Some(UpdateConfigResponse { warnings, ..answer(STATUS_PENDING, CONFIRMATION_NEEDED) })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::config::*;

    const RO: &str = READ_ONLY_FAILED;

    /// The rules of the paper session of 17/10/2026, for the parts the
    /// cases read.
    fn paper() -> ConfigRules {
        ConfigRules {
            messages: vec![
                MessageRule { id: 118, warning: None },
                MessageRule {
                    id: 605,
                    warning: Some(("Cryptocurrency order confirmation for API".into(), "Cryptocurrency order confirmation".into())),
                },
            ],
            settings_off: vec!["letApiAccountRequestsSwitchSubscription"],
            algorithms: vec!["SMART".into()],
            pre_open_reroute: true,
            dark_pools: true,
        }
    }

    fn req(id: Option<i32>) -> UpdateConfigRequest {
        UpdateConfigRequest { req_id: id, ..Default::default() }
    }

    fn settings(id: i32, s: ApiSettingsConfig) -> UpdateConfigRequest {
        UpdateConfigRequest { api: Some(ApiConfig { settings: Some(s), precautions: None }), ..req(Some(id)) }
    }

    fn msg(id: Option<i32>, enabled: Option<bool>) -> MessageConfig {
        MessageConfig { id, enabled, ..Default::default() }
    }

    fn check(r: &UpdateConfigRequest) -> Option<(i32, String, String, Vec<String>, Vec<Option<i32>>)> {
        update_config_check(r, &paper()).map(|a| (
            a.req_id.unwrap(), a.status.unwrap(), a.message.unwrap(), a.errors,
            a.warnings.iter().map(|w| w.message_id).collect(),
        ))
    }

    fn err(id: i32, message: &str, errors: &[&str]) -> Option<(i32, String, String, Vec<String>, Vec<Option<i32>>)> {
        Some((id, "error".into(), message.into(), errors.iter().map(|e| e.to_string()).collect(), vec![]))
    }

    #[test]
    fn requests_that_change_nothing_or_only_what_can_change_pass() {
        // c01, c02, c04 (empty lock-and-exit), c15, c16, c17, d03, d06-d08, d13, c14.
        assert_eq!(check(&req(Some(7410))), None);
        assert_eq!(check(&req(None)), None);
        assert_eq!(check(&UpdateConfigRequest { lock_and_exit: Some(Default::default()), ..req(Some(7413)) }), None);
        assert_eq!(check(&settings(7424, ApiSettingsConfig {
            socket_port: Some(4002), create_api_message_log_file: Some(true), logging_level: Some("system".into()),
            bulk_data_timeout: Some(30), ..Default::default()
        })), None);
        let seek = OrdersConfig { smart_routing: Some(OrdersSmartRoutingConfig { seek_price_improvement: Some(false), ..Default::default() }) };
        assert_eq!(check(&UpdateConfigRequest { orders: Some(seek), ..req(Some(7425)) }), None);
        assert_eq!(check(&UpdateConfigRequest { messages: vec![msg(Some(118), None)], ..req(Some(7426)) }), None);
        assert_eq!(check(&settings(7503, ApiSettingsConfig { send_forex_data_in_compatibility_mode: Some(false), ..Default::default() })), None);
        assert_eq!(check(&settings(7506, ApiSettingsConfig {
            encode_api_messages: Some("ASCII7".into()), logging_level: Some("SYSTEM".into()),
            send_instrument_timezone: Some("INSTRUMENT".into()), ..Default::default()
        })), None);
        assert_eq!(check(&UpdateConfigRequest { reset_api_order_sequence: Some(false), ..req(Some(7513)) }), None);
        let accepted = UpdateConfigRequest {
            messages: vec![msg(Some(605), Some(false))],
            accepted_warnings: vec![UpdateConfigWarning { message_id: Some(605), ..Default::default() }],
            ..req(Some(7423))
        };
        assert_eq!(check(&accepted), None);
    }

    #[test]
    fn a_read_only_field_is_refused_first_in_the_reference_order() {
        // c03, c05, c06, c07, d10, d11, c20, d09, c10.
        let lock = |t: Option<&str>, p: Option<&str>, k: Option<&str>| LockAndExitConfig {
            auto_logoff_time: t.map(Into::into), auto_logoff_period: p.map(Into::into), auto_logoff_type: k.map(Into::into),
        };
        assert_eq!(check(&UpdateConfigRequest { lock_and_exit: Some(lock(Some("11:45"), None, None)), ..req(Some(7412)) }),
                   err(7412, RO, &["configuration.lockAndExit.autoLogoffTime"]));
        assert_eq!(check(&UpdateConfigRequest {
            api: Some(ApiConfig { precautions: Some(Default::default()), settings: None }), ..req(Some(7414))
        }), err(7414, RO, &["configuration.api.precautions"]));
        assert_eq!(check(&settings(7415, ApiSettingsConfig { allow_localhost_only: Some(true), ..Default::default() })),
                   err(7415, RO, &["configuration.api.settings.allowLocalhostOnly"]));
        let titled = MessageConfig { id: Some(118), title: Some("x".into()), ..Default::default() };
        assert_eq!(check(&UpdateConfigRequest { messages: vec![titled.clone()], ..req(Some(7416)) }),
                   err(7416, RO, &["configuration.messages.title"]));
        assert_eq!(check(&UpdateConfigRequest { lock_and_exit: Some(lock(Some("11:45"), Some("PM"), Some("logoff"))), ..req(Some(7510)) }),
                   err(7510, RO, &["configuration.lockAndExit.autoLogoffTime", "configuration.lockAndExit.autoLogoffPeriod",
                                   "configuration.lockAndExit.autoLogoffType"]));
        let all_three = MessageConfig {
            id: Some(118), title: Some("c".into()), message: Some("b".into()), default_action: Some("a".into()), enabled: None,
        };
        assert_eq!(check(&UpdateConfigRequest { messages: vec![all_three], ..req(Some(7511)) }),
                   err(7511, RO, &["configuration.messages.title"]));
        let c20 = UpdateConfigRequest {
            lock_and_exit: Some(lock(None, Some("PM"), None)),
            messages: vec![titled, MessageConfig { id: Some(118), message: Some("y".into()), default_action: Some("x".into()), ..Default::default() }],
            api: Some(ApiConfig { precautions: None, settings: Some(ApiSettingsConfig { allow_localhost_only: Some(true), ..Default::default() }) }),
            ..req(Some(7429))
        };
        assert_eq!(check(&c20), err(7429, RO, &["configuration.lockAndExit.autoLogoffPeriod",
            "configuration.api.settings.allowLocalhostOnly", "configuration.messages.title", "configuration.messages.message"]));
        let d09 = UpdateConfigRequest {
            api: Some(ApiConfig {
                precautions: Some(ApiPrecautionsConfig { bypass_bond_warning: Some(true), ..Default::default() }),
                settings: Some(ApiSettingsConfig { socket_port: Some(80), allow_localhost_only: Some(true), ..Default::default() }),
            }),
            ..req(Some(7509))
        };
        assert_eq!(check(&d09), err(7509, RO, &["configuration.api.settings.allowLocalhostOnly", "configuration.api.precautions"]));
        let c10 = UpdateConfigRequest {
            lock_and_exit: Some(lock(None, None, Some("logoff"))),
            ..settings(7419, ApiSettingsConfig { logging_level: Some("bogus".into()), ..Default::default() })
        };
        assert_eq!(check(&c10), err(7419, RO, &["configuration.lockAndExit.autoLogoffType"]));
    }

    #[test]
    fn invalid_values_are_listed_in_the_reference_order() {
        // c08, c09, d01, d02, d04, d05, d12, d14, c11, c12, c21.
        let v = VALIDATION_FAILED;
        assert_eq!(check(&settings(7417, ApiSettingsConfig { logging_level: Some("bogus".into()), ..Default::default() })),
                   err(7417, v, &["configuration.api.settings.loggingLevel"]));
        assert_eq!(check(&settings(7418, ApiSettingsConfig {
            historical_data_max_size: Some(3), send_instrument_timezone: Some("bogus".into()),
            logging_level: Some("bogus".into()), encode_api_messages: Some("bogus".into()), ..Default::default()
        })), err(7418, v, &["configuration.api.settings.encodeApiMessages", "configuration.api.settings.loggingLevel",
                            "configuration.api.settings.sendInstrumentTimezone", "configuration.api.settings.historicalDataMaxSize"]));
        for (id, port) in [(7501, 80), (7502, 70000)] {
            assert_eq!(check(&settings(id, ApiSettingsConfig { socket_port: Some(port), ..Default::default() })),
                       err(id, v, &["configuration.api.settings.socketPort"]));
        }
        assert_eq!(check(&settings(7504, ApiSettingsConfig { let_api_account_requests_switch_subscription: Some(false), ..Default::default() })),
                   err(7504, v, &["configuration.api.settings.letApiAccountRequestsSwitchSubscription"]));
        assert_eq!(check(&settings(7505, ApiSettingsConfig { master_client_id: Some(-1), ..Default::default() })),
                   err(7505, v, &["configuration.api.settings.masterClientId"]));
        assert_eq!(check(&settings(7514, ApiSettingsConfig { option_exercise_request_type: Some("bogus".into()), ..Default::default() })),
                   err(7514, v, &["configuration.api.settings.optionExerciseRequestType"]));
        let algo = OrdersConfig { smart_routing: Some(OrdersSmartRoutingConfig { default_algorithm: Some("bogus".into()), ..Default::default() }) };
        assert_eq!(check(&UpdateConfigRequest { orders: Some(algo), ..req(Some(7512)) }),
                   err(7512, v, &["configuration.orders.smartRouting.defaultAlgorithm"]));
        assert_eq!(check(&UpdateConfigRequest { messages: vec![msg(None, Some(true))], ..req(Some(7420)) }),
                   err(7420, v, &["configuration.messages: Message configuration missing required 'id' field"]));
        assert_eq!(check(&UpdateConfigRequest { messages: vec![msg(Some(999999), Some(true))], ..req(Some(7421)) }),
                   err(7421, v, &["configuration.messages: Message with id 999999 not found"]));
        let c21 = UpdateConfigRequest {
            messages: vec![msg(None, Some(true)), msg(Some(999999), Some(true))],
            ..settings(7430, ApiSettingsConfig { historical_data_max_size: Some(3), ..Default::default() })
        };
        assert_eq!(check(&c21), err(7430, v, &["configuration.api.settings.historicalDataMaxSize",
            "configuration.messages: Message configuration missing required 'id' field",
            "configuration.messages: Message with id 999999 not found"]));
    }

    #[test]
    fn disabling_a_confirmation_waits_for_its_warning_unless_something_is_refused() {
        // c13, c18, d16 pending; c19, d15 refused.
        let pending = |id: i32| Some((id, "pending".to_string(), CONFIRMATION_NEEDED.to_string(), vec![], vec![Some(605)]));
        assert_eq!(check(&UpdateConfigRequest { messages: vec![msg(Some(605), Some(false))], ..req(Some(7422)) }), pending(7422));
        let c18 = UpdateConfigRequest {
            messages: vec![msg(Some(605), Some(false))],
            ..settings(7427, ApiSettingsConfig { logging_level: Some("system".into()), ..Default::default() })
        };
        assert_eq!(check(&c18), pending(7427));
        let d16 = UpdateConfigRequest {
            messages: vec![msg(Some(605), Some(false))],
            accepted_warnings: vec![UpdateConfigWarning { message_id: Some(444), ..Default::default() }],
            ..req(Some(7516))
        };
        assert_eq!(check(&d16), pending(7516));
        let full = update_config_check(&d16, &paper()).unwrap().warnings;
        assert_eq!(full, [UpdateConfigWarning {
            message_id: Some(605),
            title: Some("Cryptocurrency order confirmation for API".into()),
            message: Some("Cryptocurrency order confirmation".into()),
        }]);
        let c19 = UpdateConfigRequest {
            messages: vec![msg(Some(605), Some(false))],
            ..settings(7428, ApiSettingsConfig { logging_level: Some("bogus".into()), ..Default::default() })
        };
        assert_eq!(check(&c19), err(7428, VALIDATION_FAILED, &["configuration.api.settings.loggingLevel"]));
        let d15 = UpdateConfigRequest { messages: vec![msg(Some(605), Some(false)), msg(Some(999999), Some(true))], ..req(Some(7515)) };
        assert_eq!(check(&d15), err(7515, VALIDATION_FAILED, &["configuration.messages: Message with id 999999 not found"]));
    }

    #[test]
    fn an_answer_without_request_id_has_the_largest_one() {
        let r = UpdateConfigRequest { lock_and_exit: Some(LockAndExitConfig { auto_logoff_type: Some("x".into()), ..Default::default() }), ..req(None) };
        assert_eq!(update_config_check(&r, &paper()).unwrap().req_id, Some(i32::MAX));
    }
}
