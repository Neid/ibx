"""ibx#655: quantities keep the Decimal as the program gave it, as the official library keeps it: every digit and
trailing zero, for the getters and for str()."""

from decimal import Decimal

from ibx import ContractDetails, Order


def test_an_order_quantity_reads_back_as_given():
    o = Order()
    o.totalQuantity = Decimal("12.345678901234567890")
    assert str(o.totalQuantity) == "12.345678901234567890"
    o.totalQuantity = Decimal("100.0")
    assert str(o.totalQuantity) == "100.0"
    o.total_quantity = "0.00000001"
    assert o.totalQuantity == Decimal("0.00000001")
    o.totalQuantity = 1.5
    assert o.totalQuantity == Decimal("1.5")
    o.totalQuantity = 7
    assert str(o.totalQuantity) == "7"


def test_a_contract_details_size_keeps_its_text():
    d = ContractDetails()
    d.minSize = Decimal("0.0100")
    d.sizeIncrement = Decimal("0.00000001")
    assert str(d.minSize) == "0.0100"
    assert str(d.sizeIncrement) == str(Decimal("0.00000001"))  # Python prints it 1E-8
    assert "0.0100" in str(d)
