"""Issue #564: the waits copied from the reference are a setting of connect(),
off by default. No server needed: the signature is read.
"""
import inspect

from ibx import EClient


def test_connect_takes_the_copied_waits_setting_off_by_default():
    param = inspect.signature(EClient.connect).parameters["copied_waits"]
    assert param.default is False
