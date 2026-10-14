# Gateway 1051 catalogs

The reference 10.51's error codes, order attributes and local order rules, for the layer A table tests (ibx#485,
ibx#604, `tests/gw_catalog.rs`), in the format of the 1040 catalogs (`tests/fixtures/gw1040/catalog/README.md`).
Every expected value of those tests comes from here, never from the ibx code.

## Files

| File | Rows | Columns |
|---|---|---|
| `error_codes.csv` | 637 | `code`, `message`, `wrapping`, `captured_text` (as in 1040) |
| `order_attributes.csv` | 336 | `tag`, `attribute`, `kind` (as in 1040) |
| `order_local_rules.csv` | 205 | `id`, `code`, `api_code`, `stage`, `text`, `condition` (as in 1040) |

Not made yet: `order_writers.csv` (waits for capture issue deepentropy/ib-agent#214). Until it exists, the 1040 file
is the only writers table. The tests read each file from 1051 when it is here and from 1040 otherwise (`version_of`
in `tests/gw_catalog/catalog.rs`, and the coverage matrix), so a 1051 table added here is taken at once (since
14/10/2026).

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

Not observed on the reference yet, and the rows ibx is not compared with: ibx#688 and the line "local order rules of
10.51" of ibx#635.

## Trademark terms

The texts and names that hold a trademark term carry the placeholder `%TM%` in its place: the messages of codes
2180, 10106, 10122, 10234, 10235 and 10247, and the attribute name of the order attribute 8015 (the same rows as in
1040). The tests read `%TM%` like `%s`: any text.
