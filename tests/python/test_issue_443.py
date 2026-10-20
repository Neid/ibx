"""ibx#443: the WSH requests get the reference's permission error from the
news sources of the logon: 10276 without the source, 10277 when it is listed
without a subscription. With the permission, the requests go to the server
and its answers come back as the reference gives them. The cancels answer
nothing."""

from ibx import EClient, EWrapper, WshEventData


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.events = []

    def wsh_meta_data(self, req_id, data_json):
        self.events.append(("meta", req_id, data_json))

    def wsh_event_data(self, req_id, data_json):
        self.events.append(("event", req_id, data_json))

    def error(self, req_id, error_code, error_string, advanced_order_reject_json=""):
        self.events.append(("error", req_id, error_code, error_string))


def connected(subscribed, unsubscribed):
    w = Recorder()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_set_news_sources(subscribed, unsubscribed)
    return c, w


def requests(c):
    data = WshEventData()
    data.conId = 265598
    c.req_wsh_meta_data(1)
    c.req_wsh_event_data(2, data)
    c.cancel_wsh_meta_data(1)
    c.cancel_wsh_event_data(2)
    # The answers come from the event loop (ibx#532).
    c._test_dispatch_once()


def test_no_source_is_not_allowed():
    # The captured paper logon (ibx#460) has no such source.
    c, w = connected(["BRFG", "DJ-N", "DJNL"], ["BZ", "DJTOP", "FLY"])
    requests(c)
    assert w.events == [
        ("error", 1, 10276, "News feed is not allowed."),
        ("error", 2, 10276, "News feed is not allowed."),
    ]


def test_listed_source_needs_a_subscription():
    c, w = connected(["BRFG"], ["WSHE"])
    requests(c)
    text = "News Feed requires permissions. Please login to Portal to subscribe."
    assert w.events == [("error", 1, 10277, text), ("error", 2, 10277, text)]


def test_subscribed_source_sends_the_requests():
    c, w = connected(["BRFG", "WSHE"], [])
    requests(c)
    assert w.events == [("error", 2, 10282, "WSH meta data not requested.")]


def test_answers_of_the_server():
    c, w = connected(["WSHE"], [])
    c.req_wsh_meta_data(1)
    # The request is made at its turn, in the event loop (ibx#532).
    c._test_dispatch_once()
    c._test_push_wsh_answer(1, True, True, '{"meta_data":{"event_types":[],"filters":[]}}')
    c.req_wsh_meta_data(5)
    c._test_dispatch_once()
    meta = '{"meta_data":{"event_types":[],"filters":[]}}'
    assert w.events == [("meta", 1, meta), ("meta", 5, meta)]
    w.events.clear()
    data = WshEventData()
    data.conId = 8314
    c.req_wsh_event_data(2, data)
    c.req_wsh_event_data(3, data)
    c._test_dispatch_once()
    c._test_push_wsh_answer(2, False, True, '{"calendar":{"events":[{"b":1,"a":2}]}}')
    c._test_dispatch_once()
    assert w.events == [("error", 3, 10281, "Duplicate WSH event data request."), ("event", 2, '[{"a":2,"b":1}]')]


def test_event_data_request_without_its_argument():
    c, w = connected([], [])
    c.req_wsh_event_data(3)
    c._test_dispatch_once()
    assert w.events == [("error", 3, 10276, "News feed is not allowed.")]


def test_wsh_event_data_values():
    data = WshEventData()
    assert (data.conId, data.totalLimit) == (2147483647, 2147483647)
    assert (data.filter, data.startDate, data.endDate) == ("", "", "")
    assert (data.fillWatchlist, data.fillPortfolio, data.fillCompetitors) == (False, False, False)
    data.fillWatchlist = True
    data.startDate = "20261001"
    data.totalLimit = 50
    assert (data.fill_watchlist, data.start_date, data.total_limit) == (True, "20261001", 50)
