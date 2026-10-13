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

Not made yet:

- `order_local_rules.csv`: needs a reading of the 1051 order checks (request decoder, first checks, order build,
  order rules, id flow, modify, cancel), as the 1040 one was read.
- `order_writers.csv`: waits for capture issue deepentropy/ib-agent#214.

Until they exist, the 1040 files are the only local rules and writers tables.

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
