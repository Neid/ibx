//! The configuration messages of session levels 219 and 221 in the form of
//! the official client library (ibx#612): it gives them as its protobuf
//! objects only. A request is read from such an object, or from any object
//! with the same attribute names (an attribute set to None is not set). An
//! answer is built as the official object when the official library is
//! installed, otherwise as a plain object with the same attributes.

use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::api::config::*;

/// The value of field `name` when it is set: `HasField` on a protobuf
/// object, not None on another one.
fn present<'py>(obj: &Bound<'py, PyAny>, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    if obj.hasattr("HasField")? {
        return if obj.call_method1("HasField", (name,))?.is_truthy()? { Ok(Some(obj.getattr(name)?)) } else { Ok(None) };
    }
    match obj.getattr(name) {
        Ok(v) if !v.is_none() => Ok(Some(v)),
        _ => Ok(None),
    }
}

fn opt_bool(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<bool>> {
    present(obj, name)?.map(|v| v.is_truthy()).transpose()
}

fn opt_i32(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<i32>> {
    match present(obj, name)? {
        Some(v) => Ok(Some(v.extract::<i32>()?)),
        None => Ok(None),
    }
}

fn opt_str(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<String>> {
    match present(obj, name)? {
        Some(v) => Ok(Some(v.str()?.to_string())),
        None => Ok(None),
    }
}

/// The items of repeated field `name` (none when absent or None).
fn items<'py>(obj: &Bound<'py, PyAny>, name: &str) -> PyResult<Vec<Bound<'py, PyAny>>> {
    match obj.getattr(name) {
        Ok(v) if !v.is_none() => v.try_iter()?.collect(),
        _ => Ok(Vec::new()),
    }
}

fn lock_and_exit(o: &Bound<'_, PyAny>) -> PyResult<LockAndExitConfig> {
    Ok(LockAndExitConfig {
        auto_logoff_time: opt_str(o, "autoLogoffTime")?,
        auto_logoff_period: opt_str(o, "autoLogoffPeriod")?,
        auto_logoff_type: opt_str(o, "autoLogoffType")?,
    })
}

fn message(o: &Bound<'_, PyAny>) -> PyResult<MessageConfig> {
    Ok(MessageConfig {
        id: opt_i32(o, "id")?,
        title: opt_str(o, "title")?,
        message: opt_str(o, "message")?,
        default_action: opt_str(o, "defaultAction")?,
        enabled: opt_bool(o, "enabled")?,
    })
}

fn precautions(o: &Bound<'_, PyAny>) -> PyResult<ApiPrecautionsConfig> {
    Ok(ApiPrecautionsConfig {
        bypass_order_precautions: opt_bool(o, "bypassOrderPrecautions")?,
        bypass_bond_warning: opt_bool(o, "bypassBondWarning")?,
        bypass_negative_yield_confirmation: opt_bool(o, "bypassNegativeYieldConfirmation")?,
        bypass_called_bond_warning: opt_bool(o, "bypassCalledBondWarning")?,
        bypass_same_action_pair_trade_warning: opt_bool(o, "bypassSameActionPairTradeWarning")?,
        bypass_flagged_accounts_warning: opt_bool(o, "bypassFlaggedAccountsWarning")?,
        bypass_price_based_volatility_warning: opt_bool(o, "bypassPriceBasedVolatilityWarning")?,
        bypass_redirect_order_warning: opt_bool(o, "bypassRedirectOrderWarning")?,
        bypass_no_overfill_protection: opt_bool(o, "bypassNoOverfillProtection")?,
        bypass_route_marketable_to_bbo: opt_bool(o, "bypassRouteMarketableToBBO")?,
    })
}

fn settings(o: &Bound<'_, PyAny>) -> PyResult<ApiSettingsConfig> {
    Ok(ApiSettingsConfig {
        read_only_api: opt_bool(o, "readOnlyApi")?,
        total_quantity_for_mutual_funds: opt_bool(o, "totalQuantityForMutualFunds")?,
        download_open_orders_on_connection: opt_bool(o, "downloadOpenOrdersOnConnection")?,
        include_virtual_fx_positions: opt_bool(o, "includeVirtualFxPositions")?,
        prepare_daily_pnl: opt_bool(o, "prepareDailyPnL")?,
        send_status_updates_for_volatility_orders: opt_bool(o, "sendStatusUpdatesForVolatilityOrders")?,
        encode_api_messages: opt_str(o, "encodeApiMessages")?,
        socket_port: opt_i32(o, "socketPort")?,
        use_negative_auto_range: opt_bool(o, "useNegativeAutoRange")?,
        create_api_message_log_file: opt_bool(o, "createApiMessageLogFile")?,
        include_market_data_in_log_file: opt_bool(o, "includeMarketDataInLogFile")?,
        expose_trading_schedule_to_api: opt_bool(o, "exposeTradingScheduleToApi")?,
        split_insured_deposit_from_cash_balance: opt_bool(o, "splitInsuredDepositFromCashBalance")?,
        send_zero_positions_for_today_only: opt_bool(o, "sendZeroPositionsForTodayOnly")?,
        let_api_account_requests_switch_subscription: opt_bool(o, "letApiAccountRequestsSwitchSubscription")?,
        use_account_groups_with_allocation_methods: opt_bool(o, "useAccountGroupsWithAllocationMethods")?,
        logging_level: opt_str(o, "loggingLevel")?,
        master_client_id: opt_i32(o, "masterClientId")?,
        bulk_data_timeout: opt_i32(o, "bulkDataTimeout")?,
        component_exch_separator: opt_str(o, "componentExchSeparator")?,
        show_forex_data_in_1_10_pips: opt_bool(o, "showForexDataIn1_10Pips")?,
        allow_forex_trading_in_1_10_pips: opt_bool(o, "allowForexTradingIn1_10Pips")?,
        round_account_values_to_nearest_whole_number: opt_bool(o, "roundAccountValuesToNearestWholeNumber")?,
        send_market_data_in_lots_for_us_stocks: opt_bool(o, "sendMarketDataInLotsForUsStocks")?,
        show_advanced_order_reject_in_ui: opt_bool(o, "showAdvancedOrderRejectInUi")?,
        reject_messages_above_max_rate: opt_bool(o, "rejectMessagesAboveMaxRate")?,
        maintain_connection_on_incorrect_fields: opt_bool(o, "maintainConnectionOnIncorrectFields")?,
        compatibility_mode_nasdaq_stocks: opt_bool(o, "compatibilityModeNasdaqStocks")?,
        send_instrument_timezone: opt_str(o, "sendInstrumentTimezone")?,
        send_forex_data_in_compatibility_mode: opt_bool(o, "sendForexDataInCompatibilityMode")?,
        maintain_and_resubmit_orders_on_reconnect: opt_bool(o, "maintainAndResubmitOrdersOnReconnect")?,
        historical_data_max_size: opt_i32(o, "historicalDataMaxSize")?,
        auto_report_netting_event_contract_trades: opt_bool(o, "autoReportNettingEventContractTrades")?,
        option_exercise_request_type: opt_str(o, "optionExerciseRequestType")?,
        allow_localhost_only: opt_bool(o, "allowLocalhostOnly")?,
        trusted_ips: items(o, "trustedIPs")?.iter().map(|v| Ok(v.str()?.to_string())).collect::<PyResult<_>>()?,
    })
}

fn api(o: &Bound<'_, PyAny>) -> PyResult<ApiConfig> {
    Ok(ApiConfig {
        precautions: present(o, "precautions")?.map(|p| precautions(&p)).transpose()?,
        settings: present(o, "settings")?.map(|s| settings(&s)).transpose()?,
    })
}

fn orders(o: &Bound<'_, PyAny>) -> PyResult<OrdersConfig> {
    let smart = |s: &Bound<'_, PyAny>| -> PyResult<OrdersSmartRoutingConfig> {
        Ok(OrdersSmartRoutingConfig {
            seek_price_improvement: opt_bool(s, "seekPriceImprovement")?,
            pre_open_reroute: opt_bool(s, "preOpenReroute")?,
            do_not_route_to_dark_pools: opt_bool(s, "doNotRouteToDarkPools")?,
            default_algorithm: opt_str(s, "defaultAlgorithm")?,
        })
    };
    Ok(OrdersConfig { smart_routing: present(o, "smartRouting")?.map(|s| smart(&s)).transpose()? })
}

fn warning(o: &Bound<'_, PyAny>) -> PyResult<UpdateConfigWarning> {
    Ok(UpdateConfigWarning {
        message_id: opt_i32(o, "messageId")?,
        title: opt_str(o, "title")?,
        message: opt_str(o, "message")?,
    })
}

/// The request of `updateConfigProtoBuf` (an `UpdateConfigRequest` of the
/// official library, or an object with its attributes).
pub(crate) fn update_config_request(o: &Bound<'_, PyAny>) -> PyResult<UpdateConfigRequest> {
    Ok(UpdateConfigRequest {
        req_id: opt_i32(o, "reqId")?,
        lock_and_exit: present(o, "lockAndExit")?.map(|l| lock_and_exit(&l)).transpose()?,
        messages: items(o, "messages")?.iter().map(message).collect::<PyResult<_>>()?,
        api: present(o, "api")?.map(|a| api(&a)).transpose()?,
        orders: present(o, "orders")?.map(|a| orders(&a)).transpose()?,
        accepted_warnings: items(o, "acceptedWarnings")?.iter().map(warning).collect::<PyResult<_>>()?,
        reset_api_order_sequence: opt_bool(o, "resetAPIOrderSequence")?,
    })
}

/// A plain object with the attributes of an official one (when the
/// official library is not installed).
fn plain<'py>(py: Python<'py>, fields: &Bound<'py, PyDict>) -> PyResult<Bound<'py, PyAny>> {
    py.import("types")?.getattr("SimpleNamespace")?.call((), Some(fields))
}

fn warning_fields<'py>(py: Python<'py>, w: &UpdateConfigWarning) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    if let Some(v) = w.message_id { d.set_item("messageId", v)?; }
    if let Some(v) = &w.title { d.set_item("title", v)?; }
    if let Some(v) = &w.message { d.set_item("message", v)?; }
    Ok(d)
}

/// The argument of `update_config_response_proto_buf`: the official
/// `UpdateConfigResponse` with the fields of `r` set, or a plain object
/// with the same attributes (unset ones None, lists empty).
pub(crate) fn update_config_response<'py>(py: Python<'py>, r: &UpdateConfigResponse) -> PyResult<Bound<'py, PyAny>> {
    let official = py.import("ibapi.protobuf.UpdateConfigResponse_pb2")
        .and_then(|m| m.getattr("UpdateConfigResponse"))
        .and_then(|response| Ok((response, py.import("ibapi.protobuf.UpdateConfigWarning_pb2")?.getattr("UpdateConfigWarning")?)));
    let d = PyDict::new(py);
    if let Some(v) = r.req_id { d.set_item("reqId", v)?; }
    if let Some(v) = &r.status { d.set_item("status", v)?; }
    if let Some(v) = &r.message { d.set_item("message", v)?; }
    d.set_item("changedFields", r.changed_fields.clone())?;
    d.set_item("errors", r.errors.clone())?;
    match official {
        Ok((response, warning)) => {
            let warnings = r.warnings.iter()
                .map(|w| warning.call((), Some(&warning_fields(py, w)?)))
                .collect::<PyResult<Vec<_>>>()?;
            d.set_item("warnings", warnings)?;
            response.call((), Some(&d))
        }
        Err(_) => {
            for name in ["reqId", "status", "message"] {
                if !d.contains(name)? { d.set_item(name, py.None())?; }
            }
            let warnings = r.warnings.iter()
                .map(|w| {
                    let f = warning_fields(py, w)?;
                    for name in ["messageId", "title", "message"] {
                        if !f.contains(name)? { f.set_item(name, py.None())?; }
                    }
                    plain(py, &f)
                })
                .collect::<PyResult<Vec<_>>>()?;
            d.set_item("warnings", warnings)?;
            plain(py, &d)
        }
    }
}
