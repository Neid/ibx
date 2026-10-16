"""ibx#654: the text form of BarData, Order, Execution, OrderAllocation and
the historical ticks is the official library's, field for field, on the
default objects and on filled ones. Offline."""
from decimal import Decimal

import pytest

ibapi = pytest.importorskip("ibapi")
from ibapi.common import BarData as OBar, HistoricalTick as OTick, HistoricalTickBidAsk as OTickBA, \
    HistoricalTickLast as OTickLast
from ibapi.execution import Execution as OExecution
from ibapi.order import Order as OOrder
from ibapi.order_state import OrderAllocation as OAllocation

import ibx


def same(ours, theirs):
    assert str(ours) == str(theirs)


def fill(obj, values):
    for name, value in values.items():
        setattr(obj, name, value)
    return obj


def test_bar_data():
    same(ibx.BarData(), OBar())
    values = {"date": "20261013 15:30:00", "open": 250.12, "high": 251.0, "low": 249.875, "close": 250.5,
              "volume": Decimal("1234"), "wap": Decimal("250.341"), "barCount": 17}
    same(fill(ibx.BarData(), values), fill(OBar(), values))


def test_historical_ticks():
    same(ibx.HistoricalTick(), OTick())
    values = {"time": 1791900000, "price": 250.12, "size": Decimal("100")}
    same(fill(ibx.HistoricalTick(), values), fill(OTick(), values))
    same(ibx.HistoricalTickLast(), OTickLast())
    values = {"time": 1791900000, "price": 250.12, "size": Decimal("3"), "exchange": "ISLAND", "specialConditions": " T"}
    same(fill(ibx.HistoricalTickLast(), values), fill(OTickLast(), values))
    same(ibx.HistoricalTickBidAsk(), OTickBA())
    values = {"time": 1791900000, "priceBid": 250.1, "priceAsk": 250.2, "sizeBid": Decimal("200"), "sizeAsk": Decimal("300")}
    same(fill(ibx.HistoricalTickBidAsk(), values), fill(OTickBA(), values))


def test_execution():
    same(ibx.Execution(), OExecution())
    values = {"execId": "0000e0d5.6700.01.01", "time": "20261013 15:30:01 Europe/Paris", "acctNumber": "DU1",
              "exchange": "ISLAND", "side": "BOT", "shares": Decimal("100"), "price": 250.12, "permId": 12345,
              "clientId": 198, "orderId": 7, "cumQty": Decimal("100"), "avgPrice": 250.12, "orderRef": "r1",
              "lastLiquidity": 2, "submitter": "user1"}
    same(fill(ibx.Execution(), values), fill(OExecution(), values))


def test_order():
    same(ibx.Order(), OOrder())
    values = {"orderId": 7, "clientId": 198, "permId": 12345, "action": "BUY", "totalQuantity": Decimal("100"),
              "orderType": "LMT", "lmtPrice": 150.5, "tif": "DAY"}
    same(fill(ibx.Order(), values), fill(OOrder(), values))


def test_order_allocation():
    same(ibx.OrderAllocation(), OAllocation())


def test_order_combo_leg():
    from ibapi.order import OrderComboLeg as OLeg
    same(ibx.OrderComboLeg(), OLeg())
    same(fill(ibx.OrderComboLeg(), {"price": 1.25}), fill(OLeg(), {"price": 1.25}))


def test_order_conditions():
    """The conditions' text forms, default and filled (the remaining point
    of ibx#654)."""
    from ibapi import order_condition as oc
    for name in ("PriceCondition", "TimeCondition", "MarginCondition", "VolumeCondition", "PercentChangeCondition"):
        same(getattr(ibx, name)(), getattr(oc, name)())
    filled = [
        ("PriceCondition", (2, 265598, "SMART", True, 150.5)),
        ("PriceCondition", (8, 265598, "SMART", False, 150.0)),
        ("PriceCondition", (12, 265598, "SMART", False, 150.0)),
        ("TimeCondition", (True, "20261016 15:30:00 US/Eastern")),
        ("MarginCondition", (False, 30)),
        ("VolumeCondition", (265598, "SMART", True, 1000000)),
        ("PercentChangeCondition", (265598, "SMART", False, 2.5)),
        ("ExecutionCondition", ("STK", "SMART", "AAPL")),
    ]
    for name, args in filled:
        same(getattr(ibx, name)(*args), getattr(oc, name)(*args))
    # A default execution condition joins None into its text: TypeError on both sides.
    with pytest.raises(TypeError):
        str(oc.ExecutionCondition())
    with pytest.raises(TypeError):
        str(ibx.ExecutionCondition())


def test_bar_data_fields():
    """BarData has the official fields only: no timezone attribute."""
    assert not hasattr(OBar(), "timezone")
    assert not hasattr(ibx.BarData(), "timezone")


def test_order_with_conditions():
    from ibapi import order_condition as oc
    values = {"orderId": 7, "action": "BUY", "totalQuantity": Decimal("100"), "orderType": "LMT", "lmtPrice": 150.5}
    ours = fill(ibx.Order(), values)
    theirs = fill(OOrder(), values)
    ours.conditions = [ibx.PriceCondition(2, 265598, "SMART", True, 150.5), ibx.TimeCondition(False, "20261016 16:00:00")]
    theirs.conditions = [oc.PriceCondition(2, 265598, "SMART", True, 150.5), oc.TimeCondition(False, "20261016 16:00:00")]
    same(ours, theirs)
