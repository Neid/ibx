"""ibx#350: the code provider of a second factor whose code is typed gets the factor and its challenge: the
security code card's image, the passcode's delivery, the server's message. No server needed: the provider is run
the way the login runs it."""

from ibx import EClient, EWrapper


def test_the_provider_gets_the_factor_and_its_challenge():
    seen = []

    def provider(challenge):
        seen.append(challenge)
        return "4711"

    c = EClient(EWrapper())
    assert c._test_code_provider(provider, "", "", kind="3", image=b"\x89PNG", message="look up") == "4711"
    assert c._test_code_provider(provider, "", "", kind="4.2", delivery="SMS") == "4711"
    assert (seen[0]["kind"], seen[0]["image"], seen[0]["message"]) == ("3", b"\x89PNG", "look up")
    assert (seen[1]["kind"], seen[1]["delivery"]) == ("4.2", "SMS")
