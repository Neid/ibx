"""ibx#662: what is read from the session's state (ticks, request parameters) comes at its place among the
queued answers, as the reference gives the messages of its connection in the order they came: a tick message
written between two answers comes between them, also when the client reads them in one pass."""

from ibx import EClient, EWrapper


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.events = []

    def tick_price(self, req_id, tick_type, price, attrib):
        self.events.append(("price", req_id, tick_type, price))

    def tick_req_params(self, req_id, min_tick, bbo_exchange, snapshot_permissions):
        self.events.append(("params", req_id))

    def head_timestamp(self, req_id, head_timestamp):
        self.events.append(("head", req_id))


def test_state_answers_come_at_their_place():
    w = Recorder()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_set_instrument_count(1)
    c._test_map_instrument(1, 0)
    c._test_push_head_timestamp(7, "20260101")
    c._test_push_tick_req_params(0, 0.01, "9c0001", 3)
    c._test_push_head_timestamp(8, "20260101")
    c._test_push_quote(0, bid=150.0)
    c._test_push_head_timestamp(9, "20260101")
    c._test_push_quote(0, bid=149.0)
    c._test_dispatch_once()
    order = [e for e in w.events if e[0] == "head" or e[0] == "params" or (e[0] == "price" and e[2] == 1)]
    assert order == [("head", 7), ("params", 1), ("head", 8), ("price", 1, 1, 150.0), ("head", 9), ("price", 1, 1, 149.0)]
