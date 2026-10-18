//! When outside regular trading hours is kept on an order (ibx#465).
//!
//! The reference drops the outside-RTH flag of an order it does not apply
//! to: the new order gets warning 2109 and no outside-RTH tag, and a replace
//! has no outside-RTH tag either. Whether it applies depends on the order
//! type, the time in force, the exchange, and two values the server gives
//! with the contract definition for the order's exchange: the market type
//! and the order-type token list. Rule observed on the reference in
//! ib-agent#199, checked there on 10 captured orders.

use crate::types::{OrderKind, OrderRequest};

/// Text of warning 2109 when outside-RTH is dropped from a new order.
pub(crate) const OUTSIDE_RTH_IGNORED: &str = "Order Event Warning:Attribute 'Outside Regular Trading Hours' \
    is ignored based on the order type and destination. PlaceOrder is now being processed.";

/// What the rule needs from the contract definition of one exchange. The
/// empty value (no definition found) keeps outside-RTH on no order, as the
/// reference with an empty list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RthTypes {
    /// Market type of the definition (for example USSTK).
    pub market_type: String,
    /// Security type of the contract (STK, OPT, ...).
    pub sec_type: String,
    pub currency: String,
    /// Order-type tokens, from the definition's order-type list `TOKEN/N` (N = 4 counts as absent).
    pub rth: bool,
    pub lth: bool,
    pub rth_only: bool,
    pub lth_only: bool,
    pub elh_only: bool,
    pub erh_only: bool,
    pub rth4mkt: bool,
    /// The definition carried an order-type list (ibx#414). Without one
    /// the pegged types are not checked.
    pub types_known: bool,
    /// The order-type keys of pegged to market and pegged to midpoint
    /// (PEGMKT; PEGMID or PEGMID2) are in the list (ibx#414).
    pub peg_mkt: bool,
    pub peg_mid: bool,
    /// The keys of market with protection (MKTPROT) and stop with
    /// protection (STPPROT) are in the list (ibx#493).
    pub mkt_prot: bool,
    pub stp_prot: bool,
    /// The keys of pegged to best (REL2MID) and of retail price
    /// improvement (RPI) are in the list (ibx#469).
    pub peg_best: bool,
    pub rpi: bool,
    /// The key of passive relative (PASSVREL) is in the list (ibx#469).
    pub passv_rel: bool,
    /// The price check key (PRICECHK) is in the list (ibx#492).
    pub price_chk: bool,
    /// The all-or-none key (AON) is in the list (ibx#263).
    pub aon: bool,
    /// The keys of allow pre-open (PREOPGRTH) and of ignore opening
    /// auction (RTHIGNOPG) are in the list (ibx#607: the AAPL list on BEST
    /// has the first, not the second, paper 12/10/2026).
    pub pre_open: bool,
    pub ignore_opening: bool,
    /// SMART is a valid exchange of the contract (the definition names it BEST),
    /// for the redirect precaution (ibx#486).
    pub smart: bool,
    /// The contract's valid exchanges, BEST named SMART; None when the
    /// definition has no list (ibx#630).
    pub valid_exchanges: Option<Vec<String>>,
    /// The security type of the contract's underlying, for a CFD
    /// (ibx#619).
    pub under_sec_type: String,
    /// The conId of the contract's underlying, 0 when it names none: an
    /// option's, a futures option's or a warrant's first order asks it
    /// (ibx#730).
    pub under_con_id: i64,
    /// What the order message of a contract other than a stock carries
    /// (ibx#666): its local symbol, its contract month, its multiplier
    /// with two decimals (1.00 when the definition has none).
    pub local_symbol: String,
    pub contract_month: String,
    pub multiplier: String,
    /// An option's, a futures option's or a warrant's right ("1" a call,
    /// "0" a put) and strike as its order message writes them, empty for
    /// other contracts (ibx#666).
    pub right: String,
    pub strike: String,
    /// The key of the contract's trading permission class (an option's
    /// "2/OPT/R"), empty for a stock (ibx#666).
    pub permission_key: String,
    /// How the reference names a structured product or a treasury bill in
    /// the text of an order message (399), after the side and, for a
    /// structured product, the quantity; empty for the other contracts,
    /// named by symbol and listing (ibx#666).
    pub order_text: String,
    /// A treasury bill's face value: its order message gives the order's
    /// amount (quantity times face value) in place of the quantity.
    pub face_value: Option<u64>,
    /// The server answered the lookup with no definition of the contract
    /// on that exchange (ibx#743); false when none came in time.
    pub not_found: bool,
}

impl RthTypes {
    /// From the definition's order-type token list (`TOKEN/N` entries) and
    /// market type.
    pub(crate) fn from_definition(tokens: &[String], market_type: &str, sec_type: &str, currency: &str) -> Self {
        let mut t = RthTypes {
            market_type: market_type.to_string(),
            sec_type: sec_type.to_string(),
            currency: currency.to_string(),
            types_known: !tokens.is_empty(),
            ..Default::default()
        };
        for token in tokens {
            let (name, n) = token.split_once('/').unwrap_or((token.as_str(), ""));
            if n == "4" { continue; }
            match name {
                "RTH" => t.rth = true,
                "LTH" => t.lth = true,
                "RTHONLY" => t.rth_only = true,
                "LTHONLY" => t.lth_only = true,
                "ELHONLY" => t.elh_only = true,
                "ERHONLY" => t.erh_only = true,
                "RTH4MKT" => t.rth4mkt = true,
                "PEGMKT" => t.peg_mkt = true,
                "PEGMID" | "PEGMID2" => t.peg_mid = true,
                "MKTPROT" => t.mkt_prot = true,
                "STPPROT" => t.stp_prot = true,
                "REL2MID" => t.peg_best = true,
                "RPI" => t.rpi = true,
                "PASSVREL" => t.passv_rel = true,
                "PRICECHK" => t.price_chk = true,
                "AON" => t.aon = true,
                "PREOPGRTH" => t.pre_open = true,
                "RTHIGNOPG" => t.ignore_opening = true,
                _ => {}
            }
        }
        t
    }
}

/// The order-type classes the rule tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RthKind {
    /// Market-like: MKT, MTL, STP, MIT, TRAIL, STP PRT, MIDPRICE, PEG MKT,
    /// PEG MID, TRAIL MIT (FUNARI is not an order type of ibx).
    pub market_like: bool,
    /// Stop or touched: STP, STP LMT, STP PRT, TRAIL, TRAIL LIMIT, LIT, MIT,
    /// TRAIL MIT, TRAIL LIT.
    pub stop_or_touched: bool,
    /// MOC or LOC.
    pub moc_loc: bool,
}

impl RthKind {
    pub(crate) fn of(kind: &OrderKind) -> Self {
        let (market_like, stop_or_touched, moc_loc) = match kind {
            OrderKind::Market | OrderKind::Mtl | OrderKind::MidPrice { .. }
            | OrderKind::PegMkt { .. } | OrderKind::PegMid { .. } => (true, false, false),
            OrderKind::Stop { .. } | OrderKind::Mit { .. } | OrderKind::StpPrt { .. }
            | OrderKind::TrailingStop { .. } | OrderKind::TrailPct { .. }
            | OrderKind::TrailMit { .. } | OrderKind::AdjustableStop { .. } => (true, true, false),
            OrderKind::StopLimit { .. } | OrderKind::TrailingStopLimit { .. }
            | OrderKind::Lit { .. } | OrderKind::TrailLit { .. } => (false, true, false),
            OrderKind::Moc | OrderKind::Loc { .. } => (false, false, true),
            OrderKind::Limit { .. } | OrderKind::MktPrt | OrderKind::SnapMkt { .. } | OrderKind::SnapMid { .. }
            | OrderKind::SnapPri { .. } | OrderKind::Rel { .. } | OrderKind::PegBench { .. }
            | OrderKind::PegBest { .. } | OrderKind::Rpi { .. } | OrderKind::PassvRel { .. }
            | OrderKind::Volatility { .. } | OrderKind::PegStk { .. } => (false, false, false),
        };
        RthKind { market_like, stop_or_touched, moc_loc }
    }
}

/// True when outside-RTH stays on the order (ib-agent#199, conditions A and
/// B). `exchange` is the order's exchange as sent (BEST for SMART); `tif` the
/// time-in-force byte. ibx sends no volatility, algo AccuDistr,
/// relative-discretionary or OMS container order, so those conditions pass.
pub(crate) fn outside_rth_applies(kind: RthKind, tif: u8, exchange: &str, types: &RthTypes) -> bool {
    // A4: a market-like type is regular-hours only on a US stock or
    // warrant, a USD option, or a contract with the RTH4MKT token.
    // A CFD on a US stock too (ibx#619; the reference 10.51, paper,
    // pre-market 13/10/2026: the AAPL CFD stop loses outside RTH and allow
    // pre-open with 2109). The reference reads the market type of the
    // underlying's definition; ibx takes a stock underlying of a USD CFD.
    let forced_rth = kind.market_like
        && ((types.sec_type == "STK" && types.market_type == "USSTK")
            || (types.sec_type == "WAR" && types.market_type == "USWAR")
            || (matches!(types.sec_type.as_str(), "OPT" | "FOP" | "IOPT") && types.currency == "USD")
            || (types.sec_type == "CFD" && types.under_sec_type == "STK" && types.currency == "USD")
            || types.rth4mkt);
    if forced_rth { return false; }
    // A6: the exchange list allows outside RTH (RTH token and a time in
    // force it applies to, or LTH token and a stop or touched type).
    if exchange == "IBKRATS" { return false; }
    let tif_ok = !kind.moc_loc
        && !matches!(tif, b'?' | b'4' | b'3')
        && (tif != b'2' || exchange == "ARCA");
    // A combo (ibx#470, its BAG definition) never keeps it: a stock combo
    // on SMART in USD lost it on every order, the first and the later ones
    // (ibx#744; the reference 10.51, paper, 17/10 and 18/10/2026, market
    // closed: 2109 on three LMT DAY orders of one session). No combo has
    // been seen keeping it.
    if types.sec_type == "BAG" {
        return false;
    } else if !((types.rth && tif_ok) || (types.lth && kind.stop_or_touched)) {
        return false;
    }
    // A9: not the overnight venues.
    if matches!(exchange, "OVERNIGHT" | "IBEOS") { return false; }
    // B: session-only lists.
    !(types.lth_only && kind.market_like)
        && !types.rth_only
        && !(types.elh_only && kind.market_like)
        && !types.erh_only
}

/// Text of warning 2109 when allow pre-open is dropped from a new order
/// (ibx#607, paper 12/10/2026: a MKT order with allowPreOpen). One 2109
/// names the last attribute dropped: allow pre-open after outside RTH.
pub(crate) const ALLOW_PRE_OPEN_IGNORED: &str = "Order Event Warning:Attribute 'Allow pre-open' \
    is ignored based on the order type and destination. PlaceOrder is now being processed.";

/// Text of error 437: outside RTH and allow pre-open on one order, where
/// both apply (ibx#607, paper 12/10/2026).
pub(crate) const PRE_OPEN_AND_OUTSIDE_RTH: &str = "Order can have only one flag Outside RTH or Allow PreOpen.";

/// True when allow pre-open, or ignore opening auction, stays on the order
/// (ibx#607, 10.51): the outside-RTH rule, the attribute's
/// key in the order-type list (no list allows it, as for all-or-none), and
/// not the time in force OVERNIGHT + DAY. Ignore opening auction is also
/// dropped on SMART with pre-open re-route, an attribute ibx never sets.
pub(crate) fn open_flag_applies(kind: RthKind, tif: u8, exchange: &str, types: &RthTypes, key: bool) -> bool {
    tif != b'b' && (!types.types_known || key) && outside_rth_applies(kind, tif, exchange, types)
}

/// The parts of a request the rule reads: the instrument (None for a
/// replace, whose instrument is the order's), the type classes, the time in
/// force, and the outside-RTH flag. None for a request that never carries
/// outside-RTH.
pub(crate) fn rth_parts(req: &mut OrderRequest) -> Option<(Option<u32>, RthKind, u8, Option<OrderKind>, &mut bool)> {
    use OrderRequest as R;
    let limit = RthKind::of(&OrderKind::Limit { price: 0 });
    match req {
        R::SubmitLimitGtc { instrument, outside_rth, .. } => Some((Some(*instrument), limit, b'1', None, outside_rth)),
        R::SubmitStopGtc { instrument, outside_rth, .. } =>
            Some((Some(*instrument), RthKind::of(&OrderKind::Stop { stop_price: 0 }), b'1', None, outside_rth)),
        R::SubmitStopLimitGtc { instrument, outside_rth, .. } =>
            Some((Some(*instrument), RthKind::of(&OrderKind::StopLimit { price: 0, stop_price: 0 }), b'1', None, outside_rth)),
        R::SubmitTrailingStopPctEx { instrument, tif, attrs, .. } =>
            Some((Some(*instrument), RthKind::of(&OrderKind::TrailPct { trail_percent: 0, trail_stop_price: 0 }), *tif, None, &mut attrs.outside_rth)),
        R::SubmitLimitEx { instrument, tif, attrs, .. }
        | R::SubmitAdaptive { instrument, tif, attrs, .. }
        | R::SubmitAlgo { instrument, tif, attrs, .. } => Some((Some(*instrument), limit, *tif, None, &mut attrs.outside_rth)),
        R::SubmitEx { instrument, kind, tif, attrs, .. } =>
            Some((Some(*instrument), RthKind::of(kind), *tif, Some(*kind), &mut attrs.outside_rth)),
        R::Modify { kind, tif, attrs, .. } => Some((None, RthKind::of(kind), *tif, Some(*kind), &mut attrs.outside_rth)),
        _ => None,
    }
}

/// The attributes of a request that carries the flags of ibx#607 (a new
/// order of the extended encoder, or a replace). None for the others,
/// which never carry them.
pub(crate) fn flag_attrs(req: &mut OrderRequest) -> Option<&mut crate::types::OrderAttrs> {
    use OrderRequest as R;
    match req {
        R::SubmitTrailingStopPctEx { attrs, .. }
        | R::SubmitLimitEx { attrs, .. }
        | R::SubmitAdaptive { attrs, .. }
        | R::SubmitAlgo { attrs, .. }
        | R::SubmitEx { attrs, .. }
        | R::Modify { attrs, .. } => Some(attrs),
        _ => None,
    }
}

/// Text of error 10257: all-or-none on an order whose contract's
/// order-type list for its exchange has no AON key (ibx#263).
pub(crate) const ALL_OR_NONE_NOT_ALLOWED: &str = "The 'All or None' order attribute may not be specified for this order.";

/// An order with all-or-none, checked against the order-type list: the
/// reference refuses it with 10257 when the contract's list for the
/// order's exchange is known and has no AON key (no list, or the empty
/// one, allows it). Some(the instrument, None for a
/// replace, whose instrument is the order's) for a request with
/// all-or-none set; None otherwise.
pub(crate) fn all_or_none_check(req: &OrderRequest) -> Option<Option<u32>> {
    match req {
        OrderRequest::Modify { attrs, .. } => attrs.all_or_none.then_some(None),
        other => {
            let (_, attrs) = other.new_order_side()?;
            attrs?.all_or_none.then(|| other.instrument())
        }
    }
}

/// Text of error 387, the reference's refusal of an order type the
/// contract's list does not allow on the order's exchange (ibx#414).
pub(crate) const UNSUPPORTED_ORDER_TYPE: &str = "Unsupported order type for this exchange and security type.";

/// A new order of a type checked against the order-type list, and its
/// instrument: the reference refuses it with 387 when the contract's
/// order-type list for its exchange lacks the type's key (the key is
/// not in the list and the type is not in the list's order types
/// either). Checked for pegged to market and pegged to
/// midpoint (ib-agent#192 B8b, ibx#414), and for market and stop with
/// protection, keys MKTPROT and STPPROT (ibx#493: neither is in the SPY
/// list on BEST), and
/// for pegged to best and retail price improvement, keys REL2MID and RPI
/// (ibx#469; captured 28/09/2026 and 07/10/2026: the SPY list on BEST has
/// no RPI key and the order is refused, the IBM list has it and the order
/// is sent), and for passive relative, key PASSVREL (no
/// recorded list has it, and the reference refused the type on every
/// route tried). A stop with protection whose stop price is 0 is not
/// checked: the reference 10.51 sends it and the server refuses it
/// (ibx#688; paper, 18/10/2026, market closed: AAPL on SMART, 387 with a
/// stop price, sent without).
pub(crate) fn pegged_type_check(req: &OrderRequest) -> Option<(u32, fn(&RthTypes) -> bool)> {
    use OrderRequest as R;
    let mkt: fn(&RthTypes) -> bool = |t| t.peg_mkt;
    let mid: fn(&RthTypes) -> bool = |t| t.peg_mid;
    let mkt_prot: fn(&RthTypes) -> bool = |t| t.mkt_prot;
    let stp_prot: fn(&RthTypes) -> bool = |t| t.stp_prot;
    let peg_best: fn(&RthTypes) -> bool = |t| t.peg_best;
    let rpi: fn(&RthTypes) -> bool = |t| t.rpi;
    let passv_rel: fn(&RthTypes) -> bool = |t| t.passv_rel;
    match req {
        R::SubmitPegMkt { instrument, .. } | R::SubmitEx { instrument, kind: OrderKind::PegMkt { .. }, .. } => Some((*instrument, mkt)),
        R::SubmitPegMid { instrument, .. } | R::SubmitEx { instrument, kind: OrderKind::PegMid { .. }, .. } => Some((*instrument, mid)),
        R::SubmitMktPrt { instrument, .. } | R::SubmitEx { instrument, kind: OrderKind::MktPrt, .. } => Some((*instrument, mkt_prot)),
        R::SubmitStpPrt { stop_price: 0, .. } | R::SubmitEx { kind: OrderKind::StpPrt { stop_price: 0 }, .. } => None,
        R::SubmitStpPrt { instrument, .. } | R::SubmitEx { instrument, kind: OrderKind::StpPrt { .. }, .. } => Some((*instrument, stp_prot)),
        R::SubmitEx { instrument, kind: OrderKind::PegBest { .. }, .. } => Some((*instrument, peg_best)),
        R::SubmitEx { instrument, kind: OrderKind::Rpi { .. }, .. } => Some((*instrument, rpi)),
        R::SubmitEx { instrument, kind: OrderKind::PassvRel { .. }, .. } => Some((*instrument, passv_rel)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aapl_best() -> RthTypes {
        // AAPL on BEST, paper 28/09/2026: RTH/1 only, market type USSTK.
        let tokens: Vec<String> = "ACTIVETIM/1,AD/5,ADJUST/1,RTH/1,AON/1".split(',').map(String::from).collect();
        RthTypes::from_definition(&tokens, "USSTK", "STK", "USD")
    }

    // ib-agent#199 section 7: the 10 captured orders (AAPL, outsideRth=true).
    #[test]
    fn captured_orders_on_a_us_stock() {
        let t = aapl_best();
        let k = |kind: OrderKind| RthKind::of(&kind);
        let cases = [
            ("STP DAY", k(OrderKind::Stop { stop_price: 1 }), b'0', "BEST", false),
            ("TRAIL amount DAY", k(OrderKind::TrailingStop { trail_amt: 1, trail_stop_price: 0 }), b'0', "BEST", false),
            ("TRAIL percent GTC", k(OrderKind::TrailPct { trail_percent: 1, trail_stop_price: 0 }), b'1', "BEST", false),
            ("STP LMT GTC", k(OrderKind::StopLimit { price: 1, stop_price: 1 }), b'1', "BEST", true),
            ("TRAIL LIMIT GTC", k(OrderKind::TrailingStopLimit { lmt_offset: 1, lmt_price: None, trail_amt: 1, trail_stop_price: 1 }), b'1', "BEST", true),
            ("MIT GTC", k(OrderKind::Mit { stop_price: 1 }), b'1', "BEST", false),
            ("LIT GTC", k(OrderKind::Lit { price: 1, stop_price: 1 }), b'1', "BEST", true),
            ("MKT DAY", k(OrderKind::Market), b'0', "BEST", false),
            ("LMT FOK", k(OrderKind::Limit { price: 1 }), b'4', "BEST", false),
            ("LMT IOC", k(OrderKind::Limit { price: 1 }), b'3', "BEST", false),
            ("LMT OPG", k(OrderKind::Limit { price: 1 }), b'2', "BEST", false),
            ("REL DAY", k(OrderKind::Rel { price: 0, offset: 1 }), b'0', "BEST", true),
            ("LMT DAY", k(OrderKind::Limit { price: 1 }), b'0', "BEST", true),
            ("LMT GTC", k(OrderKind::Limit { price: 1 }), b'1', "BEST", true),
        ];
        for (label, kind, tif, exch, keep) in cases {
            assert_eq!(outside_rth_applies(kind, tif, exch, &t), keep, "{label}");
        }
    }

    // ibx#744 (the reference 10.51, paper, 18/10/2026, market closed): a
    // stock combo on SMART loses outside RTH on its first and later LMT DAY
    // orders, whose time in force would keep it on a stock.
    #[test]
    fn a_combo_never_keeps_outside_rth() {
        let bag = RthTypes { sec_type: "BAG".into(), currency: "USD".into(), rth: true, lth: true, ..Default::default() };
        for kind in [OrderKind::Limit { price: 1 }, OrderKind::StopLimit { price: 1, stop_price: 1 }] {
            assert!(!outside_rth_applies(RthKind::of(&kind), b'0', "BEST", &bag), "{kind:?}");
            assert!(!outside_rth_applies(RthKind::of(&kind), b'1', "BEST", &bag), "{kind:?}");
        }
    }

    // ibx#619 (the reference 10.51, paper, pre-market 13/10/2026, the
    // lists of `.tmp/619/gateway_L.log`): the AAPL CFD stop loses outside
    // RTH and allow pre-open, its limit keeps them; the MES future's list
    // has no pre-open key, so its stop loses allow pre-open.
    #[test]
    fn a_cfd_on_a_us_stock_and_a_future_as_the_reference() {
        let tokens = |list: &str| list.split(',').map(String::from).collect::<Vec<_>>();
        let mut cfd = RthTypes::from_definition(&tokens("ACTIVETIM/1,PREOPGRTH/1,RTH/1,STP/1,LMT/3"), "", "CFD", "USD");
        cfd.under_sec_type = "STK".into();
        let stop = RthKind::of(&OrderKind::Stop { stop_price: 1 });
        let lmt = RthKind::of(&OrderKind::Limit { price: 1 });
        assert!(!outside_rth_applies(stop, b'0', "BEST", &cfd));
        assert!(!open_flag_applies(stop, b'0', "BEST", &cfd, cfd.pre_open));
        assert!(open_flag_applies(lmt, b'0', "BEST", &cfd, cfd.pre_open));
        // A CFD whose underlying is not a stock is not forced.
        let index_cfd = RthTypes { under_sec_type: "IND".into(), ..cfd.clone() };
        assert!(outside_rth_applies(stop, b'0', "BEST", &index_cfd));
        let fut = RthTypes::from_definition(&tokens("ACTIVETIM/1,LTH/1,STP/1,LMT/3"), "USFUT", "FUT", "USD");
        assert!(!open_flag_applies(stop, b'0', "CME", &fut, fut.pre_open));
    }

    // Parts of the rule not observed yet on paper.
    #[test]
    fn rule_not_observed_on_paper() {
        let t = aapl_best();
        let lmt = RthKind::of(&OrderKind::Limit { price: 1 });
        // OPG is kept only on ARCA.
        assert!(outside_rth_applies(lmt, b'2', "ARCA", &t));
        // No list found: nothing keeps outside-RTH.
        assert!(!outside_rth_applies(lmt, b'0', "BEST", &RthTypes::default()));
        // Overnight venues and IBKRATS drop it.
        assert!(!outside_rth_applies(lmt, b'0', "OVERNIGHT", &t));
        assert!(!outside_rth_applies(lmt, b'0', "IBKRATS", &t));
        // MOC / LOC.
        assert!(!outside_rth_applies(RthKind::of(&OrderKind::Loc { price: 1 }), b'0', "BEST", &t));
        // A market-like type on a stock that is not a US stock keeps it
        // unless the list has RTH4MKT.
        let de = RthTypes::from_definition(&["RTH/1".to_string()], "DESTK", "STK", "EUR");
        let stp = RthKind::of(&OrderKind::Stop { stop_price: 1 });
        assert!(outside_rth_applies(stp, b'0', "BEST", &de));
        let de4 = RthTypes::from_definition(&["RTH/1".to_string(), "RTH4MKT/1".to_string()], "DESTK", "STK", "EUR");
        assert!(!outside_rth_applies(stp, b'0', "BEST", &de4));
        // LTH lets a stop or touched type through without RTH.
        let lth = RthTypes::from_definition(&["LTH/1".to_string()], "DESTK", "STK", "EUR");
        assert!(outside_rth_applies(stp, b'0', "BEST", &lth));
        assert!(!outside_rth_applies(lmt, b'0', "BEST", &lth));
        // A token with number 4 counts as absent; RTHONLY drops it.
        assert!(!outside_rth_applies(lmt, b'0', "BEST", &RthTypes::from_definition(&["RTH/4".to_string()], "USSTK", "STK", "USD")));
        assert!(!outside_rth_applies(lmt, b'0', "BEST", &RthTypes::from_definition(&["RTH/1".to_string(), "RTHONLY/1".to_string()], "USSTK", "STK", "USD")));
    }
}
