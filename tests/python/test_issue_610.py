"""ibx#610 (reqConfig, session level 219) and the rest of ibx#612 (updateConfig, level 221) in the Python client.

At the level ibx answers (214) both requests are refused with 503, as the official client library refuses them on a
session of that level. From levels 219 and 221 (reached here through test hooks, as ibx answers 214 until every
level up to 225 is handled, ibx#602), as decided on 18/10/2026 (option 3 of ibx#610):

- reqConfig gives the paper installation's configuration observed on the reference 10.51 on 15/10/2026 (paper,
  market closed, fresh process, the official client at level 225);
- an update that passes the reference's checks is kept and answered "success" for a setting that means nothing
  without the reference's installation, or for the value ibx follows; a value ibx cannot follow is answered
  "error" with the field named, and nothing changes.

The answers come from the event loop, never inside the request.
"""

import os
import sys
import types

import pytest

from ibx import EClient, EWrapper

UNSUPPORTED = "The TWS is out of date and must be upgraded.  It does not support config requests."
FIXTURE = os.path.join(os.path.dirname(__file__), "..", "fixtures", "gw1051", "config", "paper_15102026.txt")


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.events = []

    def error(self, req_id, error_code, error_string, advanced_order_reject_json=""):
        self.events.append(("error", req_id, error_code, error_string))

    def config_response_proto_buf(self, config_response_proto):
        self.events.append(("config", config_response_proto))

    def update_config_response_proto_buf(self, update_config_response_proto):
        self.events.append(("update", update_config_response_proto))


def connected():
    w = Recorder()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_apply_logon(None, "APIELOG,SECDEFTA", 199, None, None)
    return c, w


def official(name):
    return getattr(pytest.importorskip(f"ibapi.protobuf.{name}_pb2"), name)


def test_below_level_219_the_request_is_refused_at_once_for_its_id():
    ConfigRequest = official("ConfigRequest")
    c, w = connected()
    c.req_config_proto_buf(ConfigRequest(reqId=7301))
    c.req_config_proto_buf(ConfigRequest())
    c.req_config_proto_buf(types.SimpleNamespace(reqId=7302))
    c.req_config_proto_buf(None)
    # The official client library's own check: before anything is sent, on the caller's thread.
    assert w.events == [("error", 7301, 503, UNSUPPORTED), ("error", -1, 503, UNSUPPORTED),
                        ("error", 7302, 503, UNSUPPORTED)]
    c._test_dispatch_once()
    assert len(w.events) == 3


def test_not_connected_gives_504_for_the_request():
    ConfigRequest = official("ConfigRequest")
    w = Recorder()
    c = EClient(w)
    c.req_config_proto_buf(ConfigRequest(reqId=7303))
    assert w.events == [("error", 7303, 504, "Not connected")]


def test_the_configuration_is_the_one_the_reference_gave_on_15_10_2026():
    ConfigRequest = official("ConfigRequest")
    ConfigResponse = official("ConfigResponse")
    from google.protobuf import text_format
    c, w = connected()
    c._test_req_config_at(225, ConfigRequest(reqId=7301))
    assert w.events == [], "nothing inside the request"
    c._test_dispatch_once()
    [(_, answer)] = w.events
    assert isinstance(answer, ConfigResponse)
    with open(FIXTURE, encoding="utf-8") as f:
        observed = text_format.Parse(f.read(), ConfigResponse())
    assert text_format.MessageToString(answer) == text_format.MessageToString(observed)
    # Without a request id the answer has 0.
    c._test_req_config_at(225, ConfigRequest())
    c._test_dispatch_once()
    assert w.events[-1][1].reqId == 0 and w.events[-1][1].HasField("reqId")


def test_an_update_is_kept_or_refused_as_decided_on_18_10_2026():
    ConfigRequest = official("ConfigRequest")
    UpdateConfigRequest = official("UpdateConfigRequest")
    c, w = connected()
    # A setting that means nothing without the reference's installation: kept, named, shown later.
    r = UpdateConfigRequest(reqId=7801)
    r.api.settings.socketPort = 4003
    r.api.settings.loggingLevel = "DETAIL"
    m = r.messages.add()
    m.id = 444
    m.enabled = False
    r.acceptedWarnings.add().messageId = 444
    c._test_update_config_at(225, r)
    # A value ibx cannot follow: error with the field named, nothing changed.
    r = UpdateConfigRequest(reqId=7802)
    r.api.settings.readOnlyApi = True
    r.api.settings.bulkDataTimeout = 60
    c._test_update_config_at(225, r)
    # The reference's own checks still come first.
    r = UpdateConfigRequest(reqId=7803)
    m = r.messages.add()
    m.id = 605
    m.enabled = False
    c._test_update_config_at(225, r)
    c._test_req_config_at(225, ConfigRequest(reqId=7804))
    assert w.events == []
    c._test_dispatch_once()
    kinds = [k for k, _ in w.events]
    assert kinds == ["update", "update", "update", "config"]
    ok, refused, pending, config = (x for _, x in w.events)
    assert (ok.reqId, ok.status, ok.message) == (7801, "success", "Configuration updated")
    assert list(ok.changedFields) == ["configuration.api.settings.socketPort",
                                      "configuration.api.settings.loggingLevel", "configuration.messages id: 444"]
    assert (refused.reqId, refused.status, refused.message) == (
        7802, "error", "Validating configuration update request has failed")
    assert list(refused.errors) == ["configuration.api.settings.readOnlyApi"] and not refused.changedFields
    assert pending.status == "pending" and [x.messageId for x in pending.warnings] == [605]
    s = config.api.settings
    assert (s.socketPort, s.loggingLevel, s.bulkDataTimeout, s.readOnlyApi) == (4003, "detail", 30, False)
    assert [x.enabled for x in config.messages if x.id == 444] == [False]


def test_without_the_official_library_the_answer_is_a_plain_object(monkeypatch):
    c, w = connected()
    monkeypatch.setitem(sys.modules, "ibapi.protobuf.ConfigResponse_pb2", None)
    c._test_req_config_at(225, types.SimpleNamespace(reqId=7305))
    c._test_dispatch_once()
    [(_, answer)] = w.events
    assert not hasattr(answer, "HasField")
    assert answer.reqId == 7305 and answer.lockAndExit.autoLogoffType == "logoff"
    assert [m.id for m in answer.messages][:3] == [118, 131, 432] and answer.messages[1].message is None
    assert answer.api.settings.masterClientId is None and answer.api.settings.trustedIPs == ["127.0.0.1"]
    assert answer.api.precautions.bypassFlaggedAccountsWarning is None
    assert answer.orders.smartRouting.defaultAlgorithm == "SMART"


def test_the_official_names_carry_the_request_and_its_answer():
    import official_names as on
    assert on.official_name("req_config_proto_buf") == "reqConfigProtoBuf"
    assert hasattr(on.EClient, "reqConfigProtoBuf")
    assert on.official_name("config_response_proto_buf") in vars(on.EWrapper)
