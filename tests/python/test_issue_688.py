"""ibx#688: orders the reference refuses as it reads them, answered by an
error callback with the order id and nothing sent (observed on paper,
17/10/2026, market closed, one far AAPL order each, a client at level 214):
a time in force that is not known, GTC with includeOvernight, a contract
month that cannot be read, a name that is no order type, SSHORTX, and a
TRAIL or TRAIL LIMIT order without its trailing amount. ibx used to fail in
the call. Offline: the client's checks, no engine."""
import pytest

import ibx

if not hasattr(ibx.EClient, "_test_connect"):
    pytest.skip("built without the test-support feature", allow_module_level=True)

UNSET = 1.7976931348623157e308
CAUSE = "Error validating request.-'PlaceOrder(3)' : cause - "


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []

    def error(self, req_id, error_code, error_string, advanced_order_reject_json=""):
        self.errors.append((req_id, error_code, error_string))


def aapl():
    c = ibx.Contract()
    c.con_id, c.symbol, c.sec_type, c.exchange, c.currency = 265598, "AAPL", "STK", "SMART", "USD"
    return c


def mes(month):
    c = ibx.Contract()
    c.symbol, c.sec_type, c.exchange, c.currency = "MES", "FUT", "CME", "USD"
    c.last_trade_date_or_contract_month = month
    return c


def lmt(**fields):
    o = ibx.Order()
    o.action, o.total_quantity, o.order_type, o.lmt_price, o.tif = "BUY", 1, "LMT", 50.0, "DAY"
    for name, value in fields.items():
        setattr(o, name, value)
    return o


CASES = {
    "A": (aapl(), lmt(tif="XYZ"), 10052, "Invalid time in force:XYZ"),
    "B": (aapl(), lmt(tif="GTC", include_overnight=True), 10052, "Invalid time in force:GTC"),
    "C": (mes("2026AB"), lmt(lmt_price=1000.0), 10372,
          "lastTradeDateOrContractMonth: The date entered is invalid. The correct format is yyyyMM for a contract month "
          "or yyyyMMdd for a date. E.g.: 202607 or 20260724."),
    "D": (aapl(), lmt(order_type="XYZ"), 321, CAUSE + "Invalid order type was entered"),
    "E": (aapl(), lmt(action="SSHORTX"), 321, CAUSE + "SSHORTX orders are not supported"),
    "G": (aapl(), lmt(order_type="TRAIL", lmt_price=0.0, aux_price=UNSET), 321,
          CAUSE + "You must enter a Trailing Amount for TRAIL Order."),
    "H": (aapl(), lmt(order_type="TRAIL LIMIT", lmt_price=0.0, lmt_price_offset=0.1, aux_price=UNSET), 321,
          CAUSE + "Please enter a trailing stop price"),
}


@pytest.mark.parametrize("case", sorted(CASES))
def test_refused_by_a_callback_nothing_sent(case):
    contract, order, code, text = CASES[case]
    wrapper = Errors()
    client = ibx.EClient(wrapper)
    client._test_connect()
    client.place_order(58119, contract, order)
    client._test_dispatch_once()
    assert wrapper.errors == [(58119, code, text)]
    assert client._test_take_order_conditions() is None, "nothing sent"
