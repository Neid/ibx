# Gateway 1040 catalogs

The reference 10.40's error codes, order attributes, local order rules and order writers, for the layer A table
tests (ibx#485, `tests/gw_catalog.rs`). Every expected value of those tests comes from here, never from the ibx code.
Made from 24/09/2026 to 03/10/2026, with the translation specs of the reference (order submit, modify, cancel, combo
and local validation). The tests now read the 1051 tables where they exist (`tests/fixtures/gw1051/catalog/`); these
stay for the 1040 recordings (all four have a 1051 table since 17/10/2026).

## Files

| File | Rows | Columns |
|---|---|---|
| `error_codes.csv` | 612 | `code`, `message` (format string, `%s` placeholders; alternative texts joined by ` \|\| `), `wrapping`, `captured_text` |
| `order_local_rules.csv` | 168 | `id` (code and a number), `code`, `api_code`, `stage`, `text`, `condition` |
| `order_writers.csv` | 973 | `msg` (`D` new order, `G` replace, `F` cancel), `pos`, `tag`, `block`, `when`, `condition` |
| `order_attributes.csv` | 328 | `tag`, `attribute`, `kind` |

### error_codes.csv

- `wrapping`: `as_is` (the code reaches the API client itself), `321` or `322` (always sent as the cause of
  321 "Error validating request" or 322 "Error processing request"), `321|as_is` (one or the other, depending on the
  request). 365 and 366 are `as_is`: both cancel requests send the code itself.
- `captured_text`: the text the gateway was seen sending when it differs from `message`: 104 "Cannot modify a filled
  order.", 329 and 462 with the new value after a period, 413 and 10167 with one final period, 443 with the parameter
  after a colon, 2174 with "Warning: " before it, 10148 "cannot". The tests take this text only.
- A line break of a message is written `\n` (two characters).
- `%SHORT_COMPNAME%`, `%SHORT_PRODNAME%` are the gateway's own placeholders (product and company names).

### order_local_rules.csv

One row per refusal the gateway makes before sending an order (and the modify and cancel refusals). `id` is
`<code>-<n>`, n counting the rows of the same code in table order (since 14/10/2026, ibx#686). `stage`: `read` (the
client gets 320 "Error reading request:" with the rule text, except the date and time codes, which keep their code),
`check` (first checks: 321 with the rule text after `.-'bH' : cause - `), `build` (API order build), `rules` (order
rules), `flow` (order id flow), `modify`, `cancel`. `text` is what the API client gets; `%s` is a value of the
request. Some `api_code` values are corrected in the 1051 table (its README).

### order_writers.csv

The fields of the three order messages in the order the gateway writes them (`pos`). A tag listed at two positions
is two alternatives, not two emissions. A `block` (the order attributes, the algo parameters, the order conditions,
the combo legs, the FA allocation...) is one position whose tags come in any order: the order of the attribute block
changes from one gateway run to the next. `when` says when the gateway writes the tag, as the tests evaluate it on
the API order:

| `when` | Meaning |
|---|---|
| `always` | always |
| `never` | never for an order placed through the API |
| `flag:<fact>` | when the API order has this fact (a limit price, an OCA group, a parent, conditions...) |
| `type_in:<types>` / `type_not_in:<types>` | for these API order types |
| `attribute` | the order attribute's own rule (not decided by the table) |
| `other` | a condition not decided by the table (order checked, presence not) |

Terms are joined by ` & `.

### order_attributes.csv

Each order attribute with its tag and the kind of value written for it: `boolean` (only `=1`, never `=0`),
`integer`, `double` (2 to 8 decimals), `decimal`, `string`, `enum`, `datetime`, `character`, `custom`, `holder`
(the order's main fields).

## Trademark terms

The texts and names that hold a trademark term carry the placeholder `%TM%` in its place: the messages of codes
2180, 10106, 10122, 10234, 10235 and 10247, the local rule of 10235, and the attribute name of one order attribute. The
tests read `%TM%` like `%s`: any text.
