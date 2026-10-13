"""ibx#532: the answers of different queues come in the order they were written, and an answer the client makes
itself comes after the answers written before it, from the event loop, never inside the request.

Offline: answers injected into a test client, no login.
"""
from ibx import EClient, EWrapper


class Seen(EWrapper):
    def __init__(self):
        super().__init__()
        self.events = []

    def error(self, req_id, code, msg, *rest):
        self.events.append(("error", req_id, code))

    def historical_data(self, req_id, bar):
        self.events.append(("bar", req_id))

    def historical_data_end(self, req_id, start, end):
        self.events.append(("bars end", req_id))

    def current_time(self, time):
        self.events.append(("time",))

    def news_providers(self, providers):
        self.events.append(("providers",))


def test_answers_in_the_order_written_and_the_clients_own_after_them():
    w = Seen()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_dispatch_once()
    w.events.clear()
    c._test_push_connection_notice(1100, "lost")
    c._test_push_historical_data(9, [("20261012", 1.0, 2.0, 0.5, 1.5, 100)], True)
    c.req_current_time()
    assert w.events == [], "never inside the request"
    c._test_push_connection_notice(1102, "restored")
    c.req_news_providers()
    c._test_dispatch_once()
    assert w.events == [
        ("error", -1, 1100), ("bar", 9), ("bars end", 9), ("time",), ("error", -1, 1102), ("providers",),
    ]
