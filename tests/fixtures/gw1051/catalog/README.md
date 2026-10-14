# Gateway 1051 catalogs

Tables of the gateway 1051 JAR for the layer A table tests (ibx#485, ibx#604, `tests/gw_catalog.rs`), in the
format of the 1040 catalogs (`tests/fixtures/gw1040/catalog/README.md`). Every expected value of those tests
comes from here, never from the ibx code.

Made on 13/10/2026 by `.tmp/604/make_catalog.py` (kept out of the repository) from the machine-made tables of the
capture repository (its error code and order attribute tables of 10.51). The same script run on the
1040 tables gives the committed 1040 `error_codes.csv` and `order_attributes.csv` byte for byte.

## Files

| File | Rows | Columns |
|---|---|---|
| `error_codes.csv` | 637 | `code`, `message`, `wrapping`, `captured_text` (as in 1040) |
| `order_attributes.csv` | 336 | `tag`, `attribute`, `kind` (as in 1040) |
| `order_local_rules.csv` | 205 | `id`, `code`, `api_code`, `stage`, `text`, `condition` (as in 1040; `id` differs, see below) |

Not made yet:

- `order_writers.csv`: waits for capture issue deepentropy/ib-agent#214.

Until it exists, the 1040 file is the only writers table. The tests read each file from 1051 when it is here and
from 1040 otherwise (`version_of` in `tests/gw_catalog/catalog.rs`, and the coverage matrix), so a 1051 table added
here is taken at once (since 14/10/2026).

### error_codes.csv

Columns, line breaks (`\n`), alternative texts (` || `) and the gateway's own placeholders as in 1040.

- `message`: the message table text of the code (its key in the 1051 message table).
- `wrapping`: from the raise sites of the code. A site that throws the validation request exception gives
  `321`, the processing one `322`; any other site sends the code itself. All sites wrapped: `321` or `322`;
  some only: `321|as_is` or `322|as_is`; none: `as_is`. Corrections of the machine table, read in the gateway's code:
  - 365 and 366 are `as_is` (as in 1040): the cancel requests send the code itself with the request id; the
    machine table marks them 322.
  - 391 and 10314 are `as_is`: the one site the machine table counts as 321 throws a request error that the
    gateway's request handler catches on its own and sends with its own code; the table's look-ahead went past
    that throw to an unrelated one further down. The 1040 catalog still has them `321|as_is`.
  - 10052 is `as_is` in the machine table itself (same throw as 391); the 1040 catalog has it `321|as_is`.
- `captured_text`: the text the gateway was captured sending when it differs from its message table. Carried
  over from 1040 for every code whose message is unchanged (104, 329, 413, 443, 462, 2174, 10148, 10167), and
  10353 with "Setting" and a final period (ibx#608, the reference 10.51 on paper, 12/10/2026). The tests take
  this text only.
- 10364 and 10365: the machine table leaves their message empty (their entries are built another way); the
  message is the text of their keys `Max_Hedging_Size_Required` and `Max_Hedging_Size_Smaller_Than_Filled` in the
  1051 message table.
- 10319, 10334, 10335 and 2147483647 have no message, as in 1040.
- 10344 has a new text and no raise site in 1051.

### order_attributes.csv

Each order attribute class (simple class name) with its tag and the kind of value its writer writes, kinds as in
1040: a value type that is its own writer gives `boolean`, `integer`, `double`, `decimal` (fraction), `string`,
`enum` (also the unit and fix enum types), `datetime` or `character`; the holder writer gives `holder`; any other
writer, and the local and duration types, give `custom`. Rows ordered by tag, then attribute name.

## Trademark terms

The texts and names that hold a trademark term carry the placeholder `%TM%` in its place: the messages of codes
2180, 10106, 10122, 10234, 10235 and 10247, and the attribute name of the order attribute 8015 (the same rows as in 1040). The
tests read `%TM%` like `%s`: any text.

### order_local_rules.csv

Read on 14/10/2026 in the code of 1051 (ibx#659): every rule of the 1040 table found again in 1051 (141 the same,
26 corrected or changed, one gone: 10087), and 38 rules new in 1051, among them those of the session levels 216 to 225.
Stages, `api_code` and `text` as in 1040, with these differences:

- `id` is `<code>-<n>`, n counting the rows of the same code in table order. It names no place in the gateway's code.
- `condition` is written in plain terms, without names of the gateway's code.
- `api_code`: the rules whose error the request handler sends as itself keep their own code (391, 10052, 10136, 10244,
  10272, 10273, 10274, 10289, 10300, 10315, 10316, 10340, 10341, 135); the 1040 table had some of them as 321 or 320.
  The hedge rules 10006 to 10010 and 10033 come as 320, as a reading error; 10027 comes as 321 at the first checks
  (code read; ibx sends 320, ibx#688).
- `text`: the handler named in a 321 text is `PlaceOrder(3)`, as a client at session level 225 gets it (seen on
  12/10/2026); the tests read it as the name ibx gives at level 214 (ibx#591).

What the code read did not settle, and the rows ibx is not compared with yet: ibx#688 and the line "local order rules
of 10.51" of ibx#635.
