"""ibx#506: `order.softDollarTier` is assigned as in the official library."""
import ibx


def test_the_soft_dollar_tier_of_an_order_is_assigned():
    order = ibx.Order()
    assert (order.softDollarTier.name, order.softDollarTier.val, order.softDollarTier.displayName) == ("", "", "")
    order.softDollarTier = ibx.SoftDollarTier("Tier1", "Value1", "First tier")
    tier = order.softDollarTier
    assert (tier.name, tier.val, tier.displayName) == ("Tier1", "Value1", "First tier")


def test_an_object_of_another_library_with_the_three_values_is_read():
    class Tier:
        name, val, displayName = "A", "B", "C"

    order = ibx.Order()
    order.softDollarTier = Tier()
    tier = order.softDollarTier
    assert (tier.name, tier.val, tier.displayName) == ("A", "B", "C")
