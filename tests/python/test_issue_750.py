"""ibx#750: the text form of Contract, ContractDetails, ComboLeg, OrderState,
CommissionAndFeesReport, NewsProvider, DepthMktDataDescription,
SmartComponent, TickAttrib, WshEventData, TagValue, SoftDollarTier,
IneligibilityReason and ContractDescription is the official library's,
field for field, on the default objects and on filled ones. Offline."""
import re
from decimal import Decimal

import pytest

ibapi = pytest.importorskip("ibapi")
from ibapi import common as ocommon
from ibapi import contract as ocontract
from ibapi.commission_and_fees_report import CommissionAndFeesReport as OReport
from ibapi.ineligibility_reason import IneligibilityReason as OReason
from ibapi.order_state import OrderAllocation as OAllocation, OrderState as OState
from ibapi.softdollartier import SoftDollarTier as OTier
from ibapi.tag_value import TagValue as OTagValue

import ibx


def same(ours, theirs):
    assert str(ours) == str(theirs)


def without_ids(text):
    """The official form of an object in a list starts with its id."""
    return re.sub(r"\d+: ", "ID: ", text)


def fill(obj, values):
    for name, value in values.items():
        setattr(obj, name, value)
    return obj


CONTRACT = {"conId": 265598, "symbol": "AAPL", "secType": "OPT", "lastTradeDateOrContractMonth": "20261120",
            "strike": 250.5, "right": "C", "multiplier": "100", "exchange": "SMART", "primaryExchange": "NASDAQ",
            "currency": "USD", "localSymbol": "AAPL  261120C00250500", "tradingClass": "AAPL",
            "includeExpired": True, "secIdType": "ISIN", "secId": "US0378331005", "description": "d",
            "issuerId": "i1", "comboLegsDescrip": "x"}

LEG = {"conId": 265598, "ratio": 2, "action": "BUY", "exchange": "SMART", "openClose": 1, "shortSaleSlot": 2,
       "designatedLocation": "L", "exemptCode": 3}


def test_defaults_of_every_class():
    pairs = [
        (ibx.Contract(), ocontract.Contract()),
        (ibx.ContractDetails(), ocontract.ContractDetails()),
        (ibx.ContractDescription(), ocontract.ContractDescription()),
        (ibx.ComboLeg(), ocontract.ComboLeg()),
        (ibx.OrderState(), OState()),
        (ibx.CommissionAndFeesReport(), OReport()),
        (ibx.NewsProvider(), ocommon.NewsProvider()),
        (ibx.DepthMktDataDescription(), ocommon.DepthMktDataDescription()),
        (ibx.SmartComponent(), ocommon.SmartComponent()),
        (ibx.TickAttrib(), ocommon.TickAttrib()),
        (ibx.TickAttribLast(), ocommon.TickAttribLast()),
        (ibx.TickAttribBidAsk(), ocommon.TickAttribBidAsk()),
        (ibx.HistogramData(), ocommon.HistogramData()),
        (ibx.WshEventData(), ocommon.WshEventData()),
        (ibx.TagValue(), OTagValue()),
        (ibx.SoftDollarTier(), OTier()),
        (ibx.IneligibilityReason(), OReason()),
    ]
    for ours, theirs in pairs:
        same(ours, theirs)


def test_contract():
    same(fill(ibx.Contract(), CONTRACT), fill(ocontract.Contract(), CONTRACT))


def test_contract_with_legs_and_delta_neutral():
    ours = fill(ibx.Contract(), CONTRACT)
    theirs = fill(ocontract.Contract(), CONTRACT)
    ours.comboLegs = [fill(ibx.ComboLeg(), LEG), ibx.ComboLeg()]
    theirs.comboLegs = [fill(ocontract.ComboLeg(), LEG), ocontract.ComboLeg()]
    same(ours, theirs)
    for c in (ours, theirs):
        c.deltaNeutralContract = fill(ocontract.DeltaNeutralContract(), {"conId": 8314, "delta": 0.5, "price": 12.25})
    same(ours, theirs)


def test_combo_leg():
    same(fill(ibx.ComboLeg(), LEG), fill(ocontract.ComboLeg(), LEG))


def test_contract_details():
    values = {"marketName": "NMS", "minTick": 0.01, "orderTypes": "LMT,MKT", "validExchanges": "SMART,ISLAND",
              "priceMagnifier": 1, "underConId": 8314, "longName": "APPLE INC", "contractMonth": "202611",
              "industry": "Technology", "category": "Computers", "subcategory": "Hardware",
              "timeZoneId": "US/Eastern", "tradingHours": "20261018:0400-20261018:2000",
              "liquidHours": "20261018:0930-20261018:1600", "evRule": "aapl", "evMultiplier": 2.5,
              "aggGroup": 1, "underSymbol": "AAPL", "underSecType": "STK", "marketRuleIds": "26,26",
              "realExpirationDate": "20261120", "stockType": "COMMON", "cusip": "037833100", "ratings": "AA",
              "descAppend": "a", "bondType": "b", "couponType": "FIXED", "callable": True, "putable": True,
              "coupon": 5.25, "convertible": True, "maturity": "20301231", "issueDate": "20201231",
              "nextOptionDate": "20271231", "nextOptionType": "CALL", "nextOptionPartial": True, "notes": "n",
              "minSize": Decimal("1"), "sizeIncrement": Decimal("0.0001"), "suggestedSizeIncrement": Decimal("100"),
              "minAlgoSize": Decimal("1"), "lastPricePrecision": Decimal("0.01"), "lastSizePrecision": Decimal("1"),
              "eventContract1": "e1", "eventContractDescription1": "ed1", "eventContractDescription2": "ed2"}
    ours = fill(ibx.ContractDetails(), values)
    theirs = fill(ocontract.ContractDetails(), values)
    fill(ours.contract, CONTRACT)
    fill(theirs.contract, CONTRACT)
    same(ours, theirs)
    # The lists print their items in the official form: the id, then the text.
    ours.secIdList = [ibx.TagValue("ISIN", "US0378331005")]
    theirs.secIdList = [OTagValue("ISIN", "US0378331005")]
    ours.ineligibilityReasonList = [ibx.IneligibilityReason("1", "no")]
    theirs.ineligibilityReasonList = [OReason("1", "no")]
    assert without_ids(str(ours)) == without_ids(str(theirs))


def test_order_state():
    values = {"status": "PreSubmitted", "initMarginBefore": "1.5", "maintMarginBefore": "2", "equityWithLoanBefore": "3",
              "initMarginChange": "4", "maintMarginChange": "5", "equityWithLoanChange": "6", "initMarginAfter": "7",
              "maintMarginAfter": "8", "equityWithLoanAfter": "9", "commissionAndFees": 1.25,
              "minCommissionAndFees": 1.0, "maxCommissionAndFees": 2.123456789, "commissionAndFeesCurrency": "USD",
              "marginCurrency": "USD", "initMarginBeforeOutsideRTH": 10.5, "maintMarginBeforeOutsideRTH": 11.0,
              "equityWithLoanBeforeOutsideRTH": 12.0, "initMarginChangeOutsideRTH": 13.0,
              "maintMarginChangeOutsideRTH": 14.0, "equityWithLoanChangeOutsideRTH": 15.0,
              "initMarginAfterOutsideRTH": 16.0, "maintMarginAfterOutsideRTH": 17.0,
              "equityWithLoanAfterOutsideRTH": 18.0, "suggestedSize": Decimal("100"), "rejectReason": "r",
              "warningText": "w", "completedTime": "20261018 10:00:00", "completedStatus": "Filled"}
    same(fill(ibx.OrderState(), values), fill(OState(), values))
    alloc = {"account": "DU1", "position": Decimal("10"), "positionDesired": Decimal("20"),
             "positionAfter": Decimal("30"), "desiredAllocQty": Decimal("5"), "allowedAllocQty": Decimal("6"),
             "isMonetary": True}
    ours = fill(ibx.OrderState(), values)
    theirs = fill(OState(), values)
    ours.orderAllocations = [fill(ibx.OrderAllocation(), alloc), ibx.OrderAllocation()]
    theirs.orderAllocations = [fill(OAllocation(), alloc), OAllocation()]
    same(ours, theirs)


def test_commission_and_fees_report():
    values = {"execId": "0000e0d5.6700.01.01", "commissionAndFees": 1.0, "currency": "USD",
              "realizedPNL": 1.7976931348623157e308, "yield_": 0.25, "yieldRedemptionDate": 20261018}
    same(fill(ibx.CommissionAndFeesReport(), values), fill(OReport(), values))


def test_small_classes():
    cases = [
        (ibx.NewsProvider, ocommon.NewsProvider, {"code": "BRFG", "name": "Briefing.com General Market Columns"}),
        (ibx.DepthMktDataDescription, ocommon.DepthMktDataDescription,
         {"exchange": "ISLAND", "secType": "STK", "listingExch": "NASDAQ", "serviceDataType": "Deep", "aggGroup": 1}),
        (ibx.SmartComponent, ocommon.SmartComponent, {"bitNumber": 4, "exchange": "ARCA", "exchangeLetter": "P"}),
        (ibx.TickAttrib, ocommon.TickAttrib, {"canAutoExecute": True, "pastLimit": False, "preOpen": True}),
        (ibx.TickAttribLast, ocommon.TickAttribLast, {"pastLimit": True, "unreported": True}),
        (ibx.TickAttribBidAsk, ocommon.TickAttribBidAsk, {"bidPastLow": True, "askPastHigh": False}),
        (ibx.HistogramData, ocommon.HistogramData, {"price": 250.125, "size": Decimal("300")}),
        (ibx.WshEventData, ocommon.WshEventData,
         {"conId": 265598, "filter": "{}", "fillWatchlist": True, "fillPortfolio": False, "fillCompetitors": True,
          "startDate": "20261001", "endDate": "20261031", "totalLimit": 10}),
        (ibx.SoftDollarTier, OTier, {"name": "n", "val": "v", "displayName": "dn"}),
    ]
    for ours, theirs, values in cases:
        same(fill(ours(), values), fill(theirs(), values))
    same(ibx.TagValue("maxPctVol", "0.1"), OTagValue("maxPctVol", "0.1"))
    same(ibx.IneligibilityReason("1", "no"), OReason("1", "no"))


def test_repr_of_list_items_is_the_official_form():
    """TagValue and IneligibilityReason: the id of the object, then its text."""
    for obj in (ibx.TagValue("a", "b"), ibx.IneligibilityReason("1", "no")):
        assert repr(obj) == f"{id(obj)}: {obj}"
