//! The configuration requests of session levels 219 and 221: the
//! configuration as the official client library gives it (its message
//! classes, one field each, `None` when not set), its request and answer
//! (ibx#610), the update request and its answer (ibx#612).

/// The lock-and-exit section (auto log-off).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LockAndExitConfig {
    pub auto_logoff_time: Option<String>,
    pub auto_logoff_period: Option<String>,
    pub auto_logoff_type: Option<String>,
}

/// One message (confirmation or warning) of the configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageConfig {
    pub id: Option<i32>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub default_action: Option<String>,
    pub enabled: Option<bool>,
}

/// The API precautions section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiPrecautionsConfig {
    pub bypass_order_precautions: Option<bool>,
    pub bypass_bond_warning: Option<bool>,
    pub bypass_negative_yield_confirmation: Option<bool>,
    pub bypass_called_bond_warning: Option<bool>,
    pub bypass_same_action_pair_trade_warning: Option<bool>,
    pub bypass_flagged_accounts_warning: Option<bool>,
    pub bypass_price_based_volatility_warning: Option<bool>,
    pub bypass_redirect_order_warning: Option<bool>,
    pub bypass_no_overfill_protection: Option<bool>,
    pub bypass_route_marketable_to_bbo: Option<bool>,
}

/// The API settings section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiSettingsConfig {
    pub read_only_api: Option<bool>,
    pub total_quantity_for_mutual_funds: Option<bool>,
    pub download_open_orders_on_connection: Option<bool>,
    pub include_virtual_fx_positions: Option<bool>,
    pub prepare_daily_pnl: Option<bool>,
    pub send_status_updates_for_volatility_orders: Option<bool>,
    pub encode_api_messages: Option<String>,
    pub socket_port: Option<i32>,
    pub use_negative_auto_range: Option<bool>,
    pub create_api_message_log_file: Option<bool>,
    pub include_market_data_in_log_file: Option<bool>,
    pub expose_trading_schedule_to_api: Option<bool>,
    pub split_insured_deposit_from_cash_balance: Option<bool>,
    pub send_zero_positions_for_today_only: Option<bool>,
    pub let_api_account_requests_switch_subscription: Option<bool>,
    pub use_account_groups_with_allocation_methods: Option<bool>,
    pub logging_level: Option<String>,
    pub master_client_id: Option<i32>,
    pub bulk_data_timeout: Option<i32>,
    pub component_exch_separator: Option<String>,
    pub show_forex_data_in_1_10_pips: Option<bool>,
    pub allow_forex_trading_in_1_10_pips: Option<bool>,
    pub round_account_values_to_nearest_whole_number: Option<bool>,
    pub send_market_data_in_lots_for_us_stocks: Option<bool>,
    pub show_advanced_order_reject_in_ui: Option<bool>,
    pub reject_messages_above_max_rate: Option<bool>,
    pub maintain_connection_on_incorrect_fields: Option<bool>,
    pub compatibility_mode_nasdaq_stocks: Option<bool>,
    pub send_instrument_timezone: Option<String>,
    pub send_forex_data_in_compatibility_mode: Option<bool>,
    pub maintain_and_resubmit_orders_on_reconnect: Option<bool>,
    pub historical_data_max_size: Option<i32>,
    pub auto_report_netting_event_contract_trades: Option<bool>,
    pub option_exercise_request_type: Option<String>,
    pub allow_localhost_only: Option<bool>,
    pub trusted_ips: Vec<String>,
}

/// The API section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiConfig {
    pub precautions: Option<ApiPrecautionsConfig>,
    pub settings: Option<ApiSettingsConfig>,
}

/// The smart routing section of the orders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrdersSmartRoutingConfig {
    pub seek_price_improvement: Option<bool>,
    pub pre_open_reroute: Option<bool>,
    pub do_not_route_to_dark_pools: Option<bool>,
    pub default_algorithm: Option<String>,
}

/// The orders section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrdersConfig {
    pub smart_routing: Option<OrdersSmartRoutingConfig>,
}

/// The configuration request. Matches `reqConfigProtoBuf`'s argument.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigRequest {
    pub req_id: Option<i32>,
}

/// The configuration. Matches `configResponseProtoBuf`'s argument.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigResponse {
    pub req_id: Option<i32>,
    pub lock_and_exit: Option<LockAndExitConfig>,
    pub messages: Vec<MessageConfig>,
    pub api: Option<ApiConfig>,
    pub orders: Option<OrdersConfig>,
}

/// A warning of an update: the message whose change needs its warning
/// accepted, or, in a request, the warning accepted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateConfigWarning {
    pub message_id: Option<i32>,
    pub title: Option<String>,
    pub message: Option<String>,
}

/// The update request. Matches `updateConfigProtoBuf`'s argument.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateConfigRequest {
    pub req_id: Option<i32>,
    pub lock_and_exit: Option<LockAndExitConfig>,
    pub messages: Vec<MessageConfig>,
    pub api: Option<ApiConfig>,
    pub orders: Option<OrdersConfig>,
    pub accepted_warnings: Vec<UpdateConfigWarning>,
    pub reset_api_order_sequence: Option<bool>,
}

/// The answer to an update. Matches `updateConfigResponseProtoBuf`'s
/// argument.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateConfigResponse {
    pub req_id: Option<i32>,
    pub status: Option<String>,
    pub message: Option<String>,
    pub changed_fields: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<UpdateConfigWarning>,
}
