"""ibx#592: the quantities of the callbacks are decimal.Decimal, as the official library gives them.

Offline: answers injected into a test client, no login.
"""
from decimal import Decimal

from ibx import EClient, EWrapper, HistogramData

NAMES = ["tick_size", "order_status", "position", "tick_by_tick_all_last", "tick_by_tick_bid_ask",
         "update_mkt_depth", "update_mkt_depth_l2", "real_time_bar", "histogram_data"]


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = {}


def _record(name):
    def f(self, *args):
        self.seen.setdefault(name, []).append(args)
    return f


for _n in NAMES:
    setattr(Recorder, _n, _record(_n))


def _client():
    w = Recorder()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    return w, c


def _is_decimal(v, text):
    assert type(v) is Decimal, f"{type(v).__name__} {v!r}"
    assert str(v) == text, f"{v!r} printed {str(v)!r}, want {text!r}"


def test_sizes_of_quotes_and_tick_by_tick_are_decimal():
    w, c = _client()
    c._test_push_quote(0, bid=100.0, ask=101.0, bid_size=5, ask_size=3, last=100.5, last_size=1)
    c._test_push_tbt_trade(0, 100.5, 7, "ISLAND")
    c._test_push_tbt_quote(0, 100.0, 101.0, 5, 3)
    c._test_dispatch_once()
    sizes = [args[2] for args in w.seen["tick_size"]]
    assert sizes
    for size in sizes:
        assert type(size) is Decimal and str(size) == str(int(size))
    last = w.seen["tick_by_tick_all_last"][0]
    _is_decimal(last[4], "7")
    bid_ask = w.seen["tick_by_tick_bid_ask"][0]
    _is_decimal(bid_ask[4], "5")
    _is_decimal(bid_ask[5], "3")


def test_order_status_filled_and_remaining_are_decimal():
    w, c = _client()
    c._test_push_fill(0, order_id=42, side="BUY", price=150.5, qty=100, remaining=0)
    c._test_dispatch_once()
    status = w.seen["order_status"][-1]
    # The official library prints 100 and 0, not 100.0 and 0.0.
    _is_decimal(status[2], "100")
    _is_decimal(status[3], "0")


def test_position_is_decimal():
    w, c = _client()
    c._test_set_position(265598, 100, 150.5)
    c._test_account_download_complete()
    c.req_positions()
    c._test_dispatch_once()
    _is_decimal(w.seen["position"][0][2], "100")


def test_depth_sizes_are_decimal():
    w, c = _client()
    c._test_push_depth(7, 0, 1, 100.25, 300.0, False)
    c._test_push_depth(7, 1, 0, 100.5, 0.5, True)
    c._test_dispatch_once()
    _is_decimal(w.seen["update_mkt_depth"][0][5], "300")
    _is_decimal(w.seen["update_mkt_depth_l2"][0][6], "0.5")


def test_real_time_bar_volume_and_wap_are_decimal():
    w, c = _client()
    c._test_push_real_time_bar(9, 1200.0, 1.2345, 17)
    c._test_dispatch_once()
    bar = w.seen["real_time_bar"][0]
    _is_decimal(bar[6], "1200")
    _is_decimal(bar[7], "1.2345")
    assert bar[8] == 17


def test_histogram_gives_histogram_data_with_a_decimal_size():
    w, c = _client()
    c._test_push_histogram(11, [(150.25, 12), (150.5, 3)])
    c._test_dispatch_once()
    items = w.seen["histogram_data"][0][1]
    assert [type(i) for i in items] == [HistogramData, HistogramData]
    assert items[0].price == 150.25
    _is_decimal(items[0].size, "12")
    assert str(items[1]) == "Price: 150.5, Size: 3"


def test_histogram_data_as_the_official_class():
    h = HistogramData()
    assert h.price == 0.0
    assert h.size == Decimal(2 ** 127 - 1)
    assert str(h) == "Price: 0, Size: "
    h.size = Decimal("2.5")
    _is_decimal(h.size, "2.5")
