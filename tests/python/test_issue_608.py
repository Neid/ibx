"""ibx#608: routeMarketableToBbo and seekPriceImprovement keep the value the caller sets (None when unset, as the
official client's order), and take the 0 or 1 of the reference's echo."""
from ibx import Order


def test_the_flags_keep_what_the_caller_sets():
    o = Order()
    assert o.routeMarketableToBbo is None and o.seekPriceImprovement is None
    o.routeMarketableToBbo = True
    o.seekPriceImprovement = False
    assert o.routeMarketableToBbo is True and o.seekPriceImprovement is False
    o.routeMarketableToBbo = 1
    o.seekPriceImprovement = 0
    assert o.routeMarketableToBbo == 1 and type(o.routeMarketableToBbo) is int
    assert o.seekPriceImprovement == 0 and type(o.seekPriceImprovement) is int
    o.routeMarketableToBbo = None
    assert o.routeMarketableToBbo is None


def test_the_what_if_type_is_unset_by_default():
    assert Order().whatIfType == 2147483647
