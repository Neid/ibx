# Gateway 1051 catalogs

The reference 10.51's error codes, order attributes, local order rules and order writers, for the layer A table
tests (ibx#485, ibx#604, `tests/gw_catalog.rs`), in the format of the 1040 catalogs
(`tests/fixtures/gw1040/catalog/README.md`). Every expected value of those tests comes from here, never from the
ibx code.

## Files

| File | Rows | Columns |
|---|---|---|
| `error_codes.csv` | 637 | `code`, `message`, `wrapping`, `captured_text` (as in 1040) |
| `order_attributes.csv` | 336 | `tag`, `attribute`, `kind` (as in 1040) |
| `order_local_rules.csv` | 218 | `id`, `code`, `api_code`, `stage`, `text`, `condition` (as in 1040) |
| `order_writers.csv` | 985 | `msg`, `pos`, `tag`, `block`, `when`, `condition` (as in 1040) |

The tests read each file from 1051 when it is here and from 1040 otherwise (`version_of` in
`tests/gw_catalog/catalog.rs`, and the coverage matrix), so a 1051 table added here is taken at once (since
14/10/2026). All four are here since 17/10/2026.

### error_codes.csv

Columns, line breaks (`\n`), alternative texts (` || `) and the gateway's own placeholders as in 1040.

- `message`: the reference's text of the code.
- `wrapping`: how the client gets the code: `321` or `322` (as the cause of 321 "Error validating request" or 322
  "Error processing request"), `as_is` (the code itself), `321|as_is` or `322|as_is` (one or the other, depending on
  the request). 365, 366, 391, 10052 and 10314 are `as_is`; the 1040 catalog has 391, 10052 and 10314 `321|as_is`.
  The column has no `320` value yet, although some codes reach the client inside 320 "Error reading request"
  (deepentropy/ib-agent#217).
- `captured_text`: the text the gateway was seen sending when it differs from `message`. Carried over from 1040 for
  every code whose message is unchanged (104, 329, 413, 443, 462, 2174, 10148, 10167), and 10353 with "Setting" and a
  final period (ibx#608, the reference 10.51 on paper, 12/10/2026). The tests take this text only.
- 10319, 10334, 10335 and 2147483647 have no message, as in 1040.

### order_attributes.csv

Each order attribute with its tag and the kind of value written for it, kinds as in 1040: `boolean`, `integer`,
`double`, `decimal` (fraction), `string`, `enum`, `datetime`, `character`, `holder` (the order's main fields) or
`custom`. Rows ordered by tag, then attribute name.

### order_local_rules.csv

The 1040 rules again for 1051 (14/10/2026, ibx#659): 141 the same, 26 corrected or changed, one gone (10087), and
38 rules new in 1051, among them those of the session levels 216 to 225. Stages, `api_code` and `text` as in 1040,
with these differences:

- `id` is `<code>-<n>`, n counting the rows of the same code in table order.
- `api_code`: these rules reach the client with their own code: 391, 10052, 10136, 10244, 10272, 10273, 10274, 10289,
  10300, 10315, 10316, 10340, 10341 and 135. The 1040 table had some of them as 321 or 320. The hedge rules 10006 to
  10010 and 10033 come as 320, and 10027 as 321 at the first checks, while ibx sends 320 (ibx#688).
- `text`: the handler named in a 321 text is `PlaceOrder(3)`, as a client at session level 225 gets it (seen on
  12/10/2026); the tests read it as the name ibx gives at level 214 (ibx#591).

Added on 18/10/2026 (ibx#688, point 3, decision of the project manager): 13 rules of the first checks that both
versions have and that were not in the 10.40 table, after the row 10340-3. A goodTillDate with a time in force
other than GTD (321-4); the open or close intent and the delta-neutral one (321-5 to 321-8); 378, which no allowed
delta-neutral order type can reach; an offset of 0 or below 0 on the types that do not take it (10115-1, 10115-2);
the limit of the requests for quote (321-9); and the FA session's places of 10136 and 10226 (10136-2, 10226-2 to
10226-4). Their ids go on from the last number of their code. The 10.40 table does not have them.

Observed on the reference 10.51 on 18/10/2026 (paper, market closed, ibx#688): 321-4, 321-5 and 321-8 as written.
10115-1 is not applied: STP, STP LMT, STP PRT, MIT, LIT and RPI with an offset of 0 are sent and the server refuses
or cancels them; PEG MKT and PEG BENCH are refused by another rule first (387, and 321 for the missing reference
contract).

Not observed on the reference yet, and the rows ibx is not compared with: ibx#688 and the line "local order rules of
10.51" of ibx#635.

### order_writers.csv

The fields of the new order, replace and cancel messages in the order 10.51 writes them, with positions, blocks and
`when` values as in 1040 (17/10/2026, ibx#604). Made by the script that gives the committed 1040 file byte for byte
from the capture repository's 10.40 writer tables, run on its 10.51 tables: each place of 10.51 takes the condition
of the 10.40 place it lines up with (the same writer, the same field, in the same order), and the places new in 10.51
take conditions read in the 10.51 tables. The attribute block holds the fields of the 10.51 attribute table. Three
condition texts carried over from 1040 are reworded in plain words (the attribute, algo and allocation blocks).

Differences from 1040:

- New order and replace: three new fields. A flag of the orders that the rebalance, investment, divestment and
  reallocation tools send for an account with a model (`never` for an API order); a third flag of the order
  conditions, whether they include the overnight session (0 or 1, written next to the two other condition flags
  whenever the order has conditions; on a replace, just before the conditions); and the user name of the login,
  written when a setting of the session asks for it (`other`).
- Replace: the API order id and client id are no longer written there (in 1040 they were written only when the logon
  lacked the API bind feature, which no recording showed).
- Price part of the new order and the replace: the option model field is gone (it was `never`).
- The market data context block is gone from the new order and the replace (it was `never`).
- The source field is written by a sub-writer of its own in the three messages. In the new order and the replace its
  first places are fixed sources (scale orders and other orders not from the API, `never`) and its last place is the
  order's own source (`always`, "Socket" for an API order). The 1040 table has the first place `always` and the last
  `other`; the tests read both the same way.

Corrections from the recordings:

- The routing field written for an order to the broker's own trading system venue is `other` (written when the order
  goes to that venue without the post-to-venue attribute), not `never`: it was seen set on three API orders of the
  recordings of 13/10/2026 (ibx#630). The 1040 table keeps `never`.

Checked against the recordings of the reference 10.51 on paper, 12/10/2026 to 16/10/2026: 282 order messages of 43
recordings (180 new orders, 10 replaces, 92 cancels), for the field order, the fields always written and the fields
never written for an API order: no mismatch after the correction above.

Not observed on the reference yet (they rest on the 10.51 tables only): an order with conditions on 10.51 (the
overnight flag of the conditions), a replace of an order with conditions, and the user name field (in none of the
recorded orders). ibx does not write the overnight flag of the conditions: the case "SubmitEx with OCA, GTD and
conditions" of `tests/gw_catalog/writers.rs` is marked open with ibx#726.

## Trademark terms

The texts and names that hold a trademark term carry the placeholder `%TM%` in its place: the messages of codes
2180, 10106, 10122, 10234, 10235 and 10247, and the attribute name of one order attribute (the same rows as in
1040). The tests read `%TM%` like `%s`: any text.
