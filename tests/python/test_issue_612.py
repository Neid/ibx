"""ibx#612: updateConfig (session level 221) in the Python client.

At the level ibx answers (214) the request is refused with 503 for its id,
or -1 without one, as the official client library refuses it on a session
of level 214 (reference 10.51, paper, 17/10/2026). The request is read from
the official protobuf object or from any object with its attributes; the
answer is given as the official object.
"""

import types

import pytest

from ibx import EClient, EWrapper

UNSUPPORTED = "The TWS is out of date and must be upgraded.  It does not support update config requests."


class Recorder(EWrapper):
    def __init__(self):
        super().__init__()
        self.events = []

    def error(self, req_id, error_code, error_string, advanced_order_reject_json=""):
        self.events.append(("error", req_id, error_code, error_string))

    def update_config_response_proto_buf(self, update_config_response_proto):
        self.events.append(("update_config_response", update_config_response_proto))


def connected():
    w = Recorder()
    c = EClient(w)
    c._test_connect("TEST123")
    c._test_apply_logon(None, "APIELOG,SECDEFTA", 199, None, None)
    return c, w


def official_request():
    return pytest.importorskip("ibapi.protobuf.UpdateConfigRequest_pb2").UpdateConfigRequest


def test_below_level_221_the_request_is_refused_at_once_for_its_id():
    UpdateConfigRequest = official_request()
    c, w = connected()
    r = UpdateConfigRequest()
    r.reqId = 7401
    c._test_update_config_at(214, r)
    c._test_update_config_at(214, UpdateConfigRequest())
    # The official client library's own check: before anything is sent,
    # on the caller's thread.
    assert w.events == [("error", 7401, 503, UNSUPPORTED), ("error", -1, 503, UNSUPPORTED)]
    c._test_dispatch_once()
    assert len(w.events) == 2


def test_a_plain_object_with_the_official_attributes_is_taken():
    c, w = connected()
    c._test_update_config_at(214, types.SimpleNamespace(reqId=7402, messages=[], acceptedWarnings=[]))
    c._test_update_config_at(214, types.SimpleNamespace(reqId=None))
    assert w.events == [("error", 7402, 503, UNSUPPORTED), ("error", -1, 503, UNSUPPORTED)]


def test_not_connected_gives_504_for_the_request():
    UpdateConfigRequest = official_request()
    w = Recorder()
    c = EClient(w)
    r = UpdateConfigRequest()
    r.reqId = 7403
    c.update_config_proto_buf(r)
    c.update_config_proto_buf(UpdateConfigRequest())
    assert w.events == [("error", 7403, 504, "Not connected"), ("error", -1, 504, "Not connected")]


def test_every_field_of_the_official_request_is_read():
    UpdateConfigRequest = official_request()
    c, _ = connected()
    r = UpdateConfigRequest()
    r.reqId = 7404
    r.lockAndExit.autoLogoffType = "logoff"
    m = r.messages.add()
    m.id = 605
    m.enabled = False
    r.messages.add().title = "x"
    r.api.precautions.SetInParent()
    r.api.settings.loggingLevel = "system"
    r.api.settings.socketPort = 4002
    r.api.settings.showForexDataIn1_10Pips = True
    r.api.settings.trustedIPs.extend(["127.0.0.1", "10.0.0.1"])
    r.orders.smartRouting.defaultAlgorithm = "SMART"
    r.acceptedWarnings.add().messageId = 605
    r.resetAPIOrderSequence = False
    text = c._test_update_config_request(r)
    for part in ["req_id: Some(7404)", 'auto_logoff_type: Some("logoff")', "auto_logoff_time: None",
                 "id: Some(605)", "enabled: Some(false)", 'title: Some("x")',
                 "precautions: Some(ApiPrecautionsConfig { bypass_order_precautions: None",
                 'logging_level: Some("system")', "socket_port: Some(4002)", "show_forex_data_in_1_10_pips: Some(true)",
                 'trusted_ips: ["127.0.0.1", "10.0.0.1"]', 'default_algorithm: Some("SMART")',
                 "accepted_warnings: [UpdateConfigWarning { message_id: Some(605)", "reset_api_order_sequence: Some(false)",
                 "read_only_api: None"]:
        assert part in text, part
    # A field never set stays unset (an empty request is all None).
    empty = c._test_update_config_request(UpdateConfigRequest())
    assert empty.startswith("UpdateConfigRequest { req_id: None, lock_and_exit: None, messages: [], api: None")


def test_the_answer_is_the_official_object():
    UpdateConfigResponse = pytest.importorskip("ibapi.protobuf.UpdateConfigResponse_pb2").UpdateConfigResponse
    c, w = connected()
    c._test_update_config_response(
        7422, "pending",
        "Configuration update requires confirmation. Please review the warnings and re-submit your request with "
        "acceptedWarnings populated.",
        [], [], [(605, "Cryptocurrency order confirmation for API", "Cryptocurrency order confirmation")])
    c._test_update_config_response(2147483647, "success", "Configuration updated",
                                   ["configuration.api.settings.socketPort"], [], [])
    (_, pending), (_, success) = w.events
    assert isinstance(pending, UpdateConfigResponse)
    assert pending.reqId == 7422 and pending.status == "pending" and not pending.changedFields
    assert [(x.messageId, x.title, x.message) for x in pending.warnings] == [
        (605, "Cryptocurrency order confirmation for API", "Cryptocurrency order confirmation")]
    assert success.reqId == 2147483647 and list(success.changedFields) == ["configuration.api.settings.socketPort"]
    assert success.HasField("reqId") and not success.warnings


def test_the_official_names_carry_the_request_and_its_answer():
    import official_names as on
    assert on.official_name("update_config_proto_buf") == "updateConfigProtoBuf"
    assert hasattr(on.EClient, "updateConfigProtoBuf")
    assert on.official_name("update_config_response_proto_buf") in vars(on.EWrapper)
