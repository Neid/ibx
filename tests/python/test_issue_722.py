"""ibx#722: a margin condition keeps its percent as given, as the official
class: an int or a float; placing an order with a float percent raises
ClientException 588 from placeOrder, nothing sent, as the official client
does at this session level (its encoder takes an int). Offline."""
import pytest

ibapi = pytest.importorskip("ibapi")
from ibapi import order_condition as oc

import ibx


@pytest.mark.parametrize("percent", [30, 30.5, 30.0, None])
def test_margin_condition_keeps_its_percent(percent):
    ours, theirs = ibx.MarginCondition(True, percent), oc.MarginCondition(True, percent)
    assert str(ours) == str(theirs)
    assert ours.percent == theirs.percent and type(ours.percent) is type(theirs.percent)
    ours.percent, theirs.percent = 25, 25
    assert str(ours) == str(theirs)


def test_a_float_percent_cannot_be_placed():
    client = ibx.EClient(ibx.EWrapper())
    for condition in (ibx.MarginCondition(True, 30.5), oc.MarginCondition(True, 30.0)):
        order = ibx.Order()
        order.action, order.orderType, order.totalQuantity, order.lmtPrice = "BUY", "LMT", 1, 100.0
        order.conditions = [condition]
        contract = ibx.Contract()
        contract.conId, contract.exchange = 265598, "SMART"
        with pytest.raises(ibx.ClientException) as raised:
            client.place_order(7, contract, order)
        assert (raised.value.code, raised.value.msg, raised.value.text) == (588, "Error encoding protobuf - ", "Error encoding conditions")
