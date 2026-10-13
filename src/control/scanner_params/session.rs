//! What the reference does to the scanner parameters between reading
//! and writing them (ibx#571).
//!
//! The text a client receives is not the whole model: items that are not
//! for clients are removed, one kind of filter field is replaced by a
//! combo with the country list, items of a vendor the session does not
//! have are removed, one instrument is added per instrument group in
//! use, and of two items that differ only in net pricing the net one is
//! removed. The steps run in the order of [`apply`].
//!
//! Where the reference fails on a document the document is refused. Its
//! result also depends on today's year (computed values) and on the walk
//! of a hash table (net pricing): both are reproduced.

use std::collections::{HashMap, HashSet};

use super::session_tables::*;
use super::tables::{CLASSES, ROOT};
use super::{ClassId, Node, Refused, Slot, Value};

/// What a session decides in the scanner parameters (ibx#571): the names
/// it has, and whether it switches every vendor item off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    names: HashSet<String>,
    no_research: bool,
}

/// Feature of the logon that switches every vendor item off.
const NO_RESEARCH: &str = "NORSRCH";

impl Session {
    /// From the feature list of the logon and the codes of the session's
    /// news providers. A feature name is an item of the comma list, cut
    /// at its first colon; the names of the session are those and the
    /// codes together.
    pub fn new<'a>(feature_list: &str, news_provider_codes: impl IntoIterator<Item = &'a str>) -> Session {
        let mut names: HashSet<String> = feature_list
            .split(',')
            .map(|item| item.split(':').next().unwrap_or(""))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        let no_research = names.contains(NO_RESEARCH);
        names.extend(news_provider_codes.into_iter().filter(|code| !code.is_empty()).map(str::to_string));
        Session { names, no_research }
    }

    /// Whether the session has a feature or a news provider of this name.
    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Whether the session switches every vendor item off.
    pub fn no_research(&self) -> bool {
        self.no_research
    }

    /// An item of this vendor stays: no vendor, or one the session has
    /// while vendor items are not switched off. A blank is a vendor.
    fn keeps(&self, vendor: Option<&str>) -> bool {
        match vendor {
            None | Some("") => true,
            Some(_) if self.no_research => false,
            Some(vendor) => self.has(vendor),
        }
    }
}

/// Apply every step to the model of a server document, in the order of
/// the reference. `this_year` is the year of the machine's clock in its
/// zone, which computed values count from. A root the reader stopped in
/// (`whole` false) skips the view of a client and the session's view: the
/// reference runs them after reading, in the step the failure ends
/// (ibx#599).
pub(super) fn apply(root: &mut Node, session: &Session, this_year: i32, whole: bool) -> Result<(), Refused> {
    if root.class() != ROOT {
        return Err(Refused);
    }
    if whole {
        client_view(root, this_year)?;
        session_view(root, session)?;
    }
    group_instruments(root)?;
    drop_net_pricing_items(root, this_year)
}

// ── The model ──

/// Position in `members` of a member, when the object has the class
/// level that declares it.
fn slot_of(node: &Node, slot: Slot) -> Option<usize> {
    let mut base = 0;
    for &level in CLASSES[node.class() as usize].chain {
        if level == slot.level {
            return Some(base + slot.at as usize);
        }
        base += CLASSES[level as usize].members.len();
    }
    None
}

/// Whether the object is of this class or of a subclass.
fn is_a(node: &Node, class: ClassId) -> bool {
    CLASSES[node.class() as usize].chain.contains(&class)
}

fn node(value: &Value) -> Option<&Node> {
    match value {
        Value::Node(node) => Some(node),
        _ => None,
    }
}

fn node_mut(value: &mut Value) -> Option<&mut Node> {
    match value {
        Value::Node(node) => Some(node),
        _ => None,
    }
}

/// A member; `None` also when the object has no such class level.
fn value(node: &Node, slot: Slot) -> Option<&Value> {
    node.members.get(slot_of(node, slot)?)
}

/// Text of a member; `None` when it has no value.
fn text(node: &Node, slot: Slot) -> Option<&str> {
    match value(node, slot)? {
        Value::Text(text) => Some(text),
        _ => None,
    }
}

/// The object a member holds.
fn child(node: &Node, slot: Slot) -> Option<&Node> {
    value(node, slot).and_then(self::node)
}

fn child_mut(node: &mut Node, slot: Slot) -> Option<&mut Node> {
    let at = slot_of(node, slot)?;
    node.members.get_mut(at).and_then(node_mut)
}

fn set(node: &mut Node, slot: Slot, value: Value) -> Result<(), Refused> {
    let at = slot_of(node, slot).ok_or(Refused)?;
    node.members[at] = value;
    Ok(())
}

fn text_value(text: &str) -> Value {
    Value::Text(text.to_string())
}

/// No value or no character. A blank is not empty.
fn empty(text: Option<&str>) -> bool {
    text.is_none_or(str::is_empty)
}

/// A text that means yes; nothing is trimmed.
fn yes(text: Option<&str>) -> bool {
    text.is_some_and(|t| YES_IGNORE_CASE.iter().any(|y| t.eq_ignore_ascii_case(y)) || YES_EXACT.contains(&t))
}

/// The three main lists are there and none is empty.
fn is_valid(root: &Node) -> bool {
    [RESPONSE_INSTRUMENT_LIST, RESPONSE_LOCATION_TREE, RESPONSE_SCAN_TYPE_LIST]
        .into_iter()
        .all(|list| child(root, list).is_some_and(|list| !list.items.is_empty()))
}

// ── What is not for clients ──

/// The view of a client (ibx#571). Nothing is done to a document that
/// is not valid.
///
/// - An instrument with a `context` leaves the instrument list; the full
///   list keeps it.
/// - A scan type leaves when `nscanOnly` is yes, when `instruments` is
///   the restricted one, or when its code has the restricted prefix.
/// - A filter of the filter list leaves when `nscanOnly` is yes, and the
///   field of a simple filter is converted ([`convert_field`]). The
///   second filter list is left as it is.
/// - The values of a filter of the filter list that asks for them are
///   computed ([`computed_values`]).
///
/// Running it again changes nothing.
fn client_view(root: &mut Node, this_year: i32) -> Result<(), Refused> {
    if !is_valid(root) {
        return Ok(());
    }
    if let Some(instruments) = child_mut(root, RESPONSE_INSTRUMENT_LIST) {
        instruments.items.retain(|item| !node(item).is_some_and(|i| is_a(i, INSTRUMENT) && !empty(text(i, INSTRUMENT_CONTEXT))));
    }

    if let Some(scan_types) = child_mut(root, RESPONSE_SCAN_TYPE_LIST) {
        let mut refused = false;
        scan_types.items.retain(|item| {
            let Some(scan) = node(item).filter(|n| is_a(n, SCAN_TYPE)) else { return true };
            if yes(text(scan, SCAN_TYPE_NSCAN_ONLY)) || text(scan, SCAN_TYPE_INSTRUMENTS) == Some(INSTRUMENTS_RESTRICTED) {
                return false;
            }
            match text(scan, SCAN_TYPE_SCAN_CODE) {
                Some(code) => !code.starts_with(SCAN_CODE_PREFIX_RESTRICTED),
                // The reference fails on a scan type without code.
                None => {
                    refused = true;
                    true
                }
            }
        });
        if refused {
            return Err(Refused);
        }
    }

    let Some(filters) = child_mut(root, RESPONSE_FILTER_LIST) else { return Ok(()) };
    for filter in filters.items.iter_mut().filter_map(node_mut) {
        if is_a(filter, SIMPLE_FILTER) {
            convert_field(filter)?;
        }
    }
    filters.items.retain(|item| !yes(node(item).and_then(|f| text(f, FILTER_NSCAN_ONLY))));
    for filter in filters.items.iter_mut().filter_map(node_mut) {
        if is_a(filter, SIMPLE_FILTER) {
            computed_values(filter, this_year)?;
        }
    }
    for filter in filters.items.iter().filter_map(node) {
        // The reference then walks the display texts of every filter,
        // which changes none of them in its default language, and fails
        // on a combo field without values: what a field converted from
        // another list than the country list is.
        let without_values = |member: &Value| node(member).is_some_and(|f| is_a(f, COMBO_FIELD) && value(f, COMBO_FIELD_VALUES) == Some(&Value::Null));
        if filter.members.iter().any(without_values) {
            return Err(Refused);
        }
    }
    Ok(())
}

/// A field with a `typeAheadList` is replaced by a converted combo field
/// that holds it as its member `field` (ibx#571). The members of the
/// combo itself are those of a fresh one, so the code, names and tooltip
/// of the field show only inside it. Its values are the country list
/// when the list named is the country one, none otherwise.
///
/// A converted field names no list itself: it is not converted again.
fn convert_field(filter: &mut Node) -> Result<(), Refused> {
    let at = slot_of(filter, SIMPLE_FILTER_FIELD).ok_or(Refused)?;
    // The reference fails on a simple filter without field.
    let field = node(&filter.members[at]).ok_or(Refused)?;
    let list = text(field, FIELD_TYPE_AHEAD_LIST);
    if empty(list) {
        return Ok(());
    }
    let countries = list == Some(TYPE_AHEAD_COUNTRY_LIST);
    let mut combo = Node::fresh(CONVERTED_COMBO_FIELD);
    let field = std::mem::replace(&mut filter.members[at], Value::Null);
    set(&mut combo, CONVERTED_FIELD, field)?;
    if countries {
        set(&mut combo, COMBO_FIELD_VALUES, Value::Node(Box::new(country_values()?)))?;
    }
    filter.members[at] = Value::Node(Box::new(combo));
    Ok(())
}

/// The values of the country list: first an entry without code that is
/// the default one, then one per country in the order of the names.
/// Display name and tooltip are both the name.
fn country_values() -> Result<Node, Refused> {
    let mut values = Node::of(COMBO_VALUES).ok_or(Refused)?;
    values.items.reserve(COUNTRIES.len() + 1);
    let entries = std::iter::once(&("", COUNTRY_ALL_LABEL)).chain(COUNTRIES).enumerate();
    for (i, &(code, name)) in entries {
        let mut entry = Node::of(COMBO_VALUE).ok_or(Refused)?;
        set(&mut entry, COMBO_VALUE_CODE, text_value(code))?;
        set(&mut entry, COMBO_VALUE_DISPLAY_NAME, text_value(name))?;
        set(&mut entry, COMBO_VALUE_TOOLTIP, text_value(name))?;
        set(&mut entry, COMBO_VALUE_DEFAULT, Value::Bool(i == 0))?;
        values.items.push(Value::Node(Box::new(entry)));
    }
    Ok(values)
}

/// Computed values (ibx#599). A simple filter whose field is a combo and
/// whose config has an item named [`VALUES_GENERATOR_KEY`] (the first
/// such item) with a value and a second value, both not empty: the value
/// is cut at `;`. When its first part is [`YEARS`], the combo's values
/// become one entry per year ([`years`]); a generator the reference
/// fails to compute leaves them as they are. A value cut into nothing,
/// or values to replace that the combo does not have, fail the reference.
fn computed_values(filter: &mut Node, this_year: i32) -> Result<(), Refused> {
    let (Some(field), Some(config)) = (child(filter, SIMPLE_FILTER_FIELD), child(filter, FILTER_CONFIG)) else { return Ok(()) };
    if !is_a(field, COMBO_FIELD) {
        return Ok(());
    }
    let named = |item: &&Node| is_a(item, CONFIG_ITEM) && text(item, CONFIG_ITEM_NAME) == Some(VALUES_GENERATOR_KEY);
    let Some(item) = config.items.iter().filter_map(node).find(named) else { return Ok(()) };
    let (Some(value), Some(labels)) = (text(item, CONFIG_ITEM_VALUE), text(item, CONFIG_ITEM_VALUE1)) else { return Ok(()) };
    if value.is_empty() || labels.is_empty() {
        return Ok(());
    }
    let parts = java_split(value, ';');
    if *parts.first().ok_or(Refused)? != YEARS {
        return Ok(());
    }
    let Some(entries) = years(&parts, &java_split(labels, ';'), this_year)? else { return Ok(()) };
    let combo = child_mut(filter, SIMPLE_FILTER_FIELD).ok_or(Refused)?;
    let values = child_mut(combo, COMBO_FIELD_VALUES).ok_or(Refused)?;
    values.items.clear();
    values.items.reserve(entries.len());
    for (code, name, default) in entries {
        let mut entry = Node::of(COMBO_VALUE).ok_or(Refused)?;
        set(&mut entry, COMBO_VALUE_CODE, Value::Text(code))?;
        set(&mut entry, COMBO_VALUE_DISPLAY_NAME, text_value(&name))?;
        set(&mut entry, COMBO_VALUE_TOOLTIP, Value::Text(name))?;
        set(&mut entry, COMBO_VALUE_DEFAULT, Value::Bool(default))?;
        values.items.push(Value::Node(Box::new(entry)));
    }
    Ok(())
}

/// First part of a generator value that asks for years.
const YEARS: &str = "years";

/// The entries of a years generator (ibx#599): code, display name and
/// whether it is the default, from the value's parts (kind, start year,
/// default code) and the second value's parts (code suffix, then the
/// display suffixes of one year and of several, cut at `/`; without a
/// second one the first serves for both). One entry per year of the
/// reference's calendar from the start year up to last year, the number
/// of years back first: "6Y" / "6 Years" down to "1Y" / "1 Year".
///
/// `None` where the reference's computation fails and leaves the values
/// as they are: a part missing, a start year that is not a whole number.
/// A start year so far off that the reference's calendar overflows is
/// not modelled: refused.
fn years(parts: &[&str], labels: &[&str], this_year: i32) -> Result<Option<Vec<YearEntry>>, Refused> {
    let (Some(start), Some(default), Some(suffix), Some(display)) = (parts.get(1), parts.get(2), labels.first(), labels.get(1)) else {
        return Ok(None);
    };
    let display = java_split(display, '/');
    let Some(&one) = display.first() else { return Ok(None) };
    let several = if display.len() == 2 { display[1] } else { one };
    let Some(start) = java_int(start) else { return Ok(None) };
    if !(-YEAR_LIMIT..=YEAR_LIMIT).contains(&start) {
        return Err(Refused);
    }
    let entry = |year: i32| {
        let back = this_year - year;
        let code = format!("{back}{suffix}");
        let name = format!("{back}{}", if back > 1 { several } else { one });
        let is_default = *default == code;
        (code, name, is_default)
    };
    Ok(Some(calendar_years(start, this_year).map(entry).collect()))
}

/// Code, display name and whether it is the default.
type YearEntry = (String, String, bool);

/// Start years beyond this are not modelled: the reference's calendar
/// overflows its milliseconds near 292 million years.
const YEAR_LIMIT: i32 = 200_000_000;

/// The years the reference's calendar shows from `start` while they are
/// before `this_year`, one year added at a time. A year of 0 or less is
/// before the era and shows as `1 - start` (0 as 1, -3 as 4); adding a
/// year counts it down to 1, then 1, 2 ... of the era follow.
fn calendar_years(start: i32, this_year: i32) -> impl Iterator<Item = i32> {
    let before_era = if start < 1 && 1 - start < this_year { 1 - start } else { 0 };
    let from = if start >= 1 {
        start
    } else if before_era > 0 {
        1
    } else {
        this_year
    };
    (1..=before_era).rev().chain(from..this_year)
}

/// A text cut at a character as the reference cuts it: empty parts at
/// the end are dropped, and an empty text is one empty part.
fn java_split(text: &str, at: char) -> Vec<&str> {
    if text.is_empty() {
        return vec![text];
    }
    let mut parts: Vec<&str> = text.split(at).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}

/// A whole number as the reference reads a start year: an optional sign,
/// then digits of any script of [`DIGIT_ZEROS`], within 32 bits.
fn java_int(text: &str) -> Option<i32> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let first = *units.first()?;
    let (negative, digits) = if first == u16::from(b'-') {
        (true, &units[1..])
    } else if first == u16::from(b'+') {
        (false, &units[1..])
    } else if first < u16::from(b'0') {
        return None;
    } else {
        (false, &units[..])
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for &unit in digits {
        let zero = DIGIT_ZEROS.iter().rev().find(|&&z| z <= unit)?;
        let digit = unit - zero;
        if digit > 9 {
            return None;
        }
        value = value * 10 + i64::from(digit);
        if value > 1 << 31 {
            return None;
        }
    }
    i32::try_from(if negative { -value } else { value }).ok()
}

/// The first digit of each run of ten decimal digits the reference's
/// runtime reads in a number (taken from that runtime, Java 25). Digits
/// outside the first 65,536 characters are not read.
#[rustfmt::skip]
const DIGIT_ZEROS: [u16; 37] = [
    0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66,
    0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50,
    0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
];

// ── What the session has ──

/// The vendor rule (ibx#571): a scan type, a filter of the filter list
/// or a sidecar template that names a vendor stays only when the session
/// has a feature or a news provider of that name, and never when the
/// session switches vendor items off.
///
/// The reference has a second pass here, by access level; it is switched
/// off in its code and nothing of it is done.
fn session_view(root: &mut Node, session: &Session) -> Result<(), Refused> {
    if let Some(scan_types) = child_mut(root, RESPONSE_SCAN_TYPE_LIST) {
        // The reference fails on an item of another class.
        if !scan_types.items.iter().all(|item| node(item).is_some_and(|n| is_a(n, SCAN_TYPE))) {
            return Err(Refused);
        }
        scan_types.items.retain(|item| session.keeps(node(item).and_then(|s| text(s, SCAN_TYPE_VENDOR))));
    }
    if let Some(filters) = child_mut(root, RESPONSE_FILTER_LIST) {
        filters.items.retain(|item| session.keeps(node(item).and_then(|f| text(f, FILTER_VENDOR))));
    }
    // The reference fails without the sidecar section.
    let sidecar = child_mut(root, RESPONSE_SIDECAR_DEFAULTS).ok_or(Refused)?;
    if let Some(templates) = child_mut(sidecar, SIDECAR_TEMPLATE_LIST) {
        templates.items.retain(|item| session.keeps(node(item).and_then(|t| text(t, TEMPLATE_VENDOR))));
    }
    // Without instruments, locations or scan types the reference reports
    // that the scanner is not available.
    if is_valid(root) {
        Ok(())
    } else {
        Err(Refused)
    }
}

// ── What a client request adds and removes ──

/// One instrument per instrument group in use (ibx#571). Walking the
/// instrument list, an instrument whose `group` is the `group` of an
/// instrument group puts that group in use (the first such group). Each
/// group in use, in the order of first use, gives one instrument: name
/// and `secType` of the group, `nscanSecType` of the group or its
/// `secType` when it has none, a fixed `type`, the rest as in a fresh
/// one. It is added at the end of the full list and of the list.
fn group_instruments(root: &mut Node) -> Result<(), Refused> {
    let (Some(instruments), Some(groups)) = (child(root, RESPONSE_INSTRUMENT_LIST), child(root, RESPONSE_INSTRUMENT_GROUP_LIST)) else {
        return Ok(());
    };
    if instruments.items.is_empty() || groups.items.is_empty() {
        return Ok(());
    }
    let mut used: Vec<&str> = Vec::new();
    let mut made: Vec<Value> = Vec::new();
    for instrument in instruments.items.iter().filter_map(node) {
        let Some(name) = text(instrument, INSTRUMENT_GROUP_OF) else { continue };
        for group in &groups.items {
            // The reference fails on a group without `group`.
            let group_name = node(group).and_then(|g| text(g, GROUP_GROUP)).ok_or(Refused)?;
            if group_name != name {
                continue;
            }
            if let (false, Some(group)) = (used.contains(&name), node(group)) {
                let member = |slot| value(group, slot).cloned().unwrap_or(Value::Null);
                let sec_type = member(GROUP_SEC_TYPE);
                let nscan_sec_type = if empty(text(group, GROUP_NSCAN_SEC_TYPE)) { sec_type.clone() } else { member(GROUP_NSCAN_SEC_TYPE) };
                let mut new = Node::of(INSTRUMENT).ok_or(Refused)?;
                set(&mut new, INSTRUMENT_NAME, member(GROUP_NAME))?;
                set(&mut new, INSTRUMENT_SEC_TYPE, sec_type)?;
                set(&mut new, INSTRUMENT_NSCAN_SEC_TYPE, nscan_sec_type)?;
                set(&mut new, INSTRUMENT_TYPE, text_value(GROUP_INSTRUMENT_TYPE))?;
                used.push(name);
                made.push(Value::Node(Box::new(new)));
            }
            break;
        }
    }
    if made.is_empty() {
        return Ok(());
    }
    // The reference fails when it has no full list to add to.
    let full = child_mut(root, RESPONSE_FULL_INSTRUMENT_LIST).ok_or(Refused)?;
    full.items.extend(made.iter().cloned());
    if let Some(instruments) = child_mut(root, RESPONSE_INSTRUMENT_LIST) {
        instruments.items.extend(made);
    }
    Ok(())
}

/// The ids of the net items that have a raw counterpart (ibx#571). An
/// item with a raw id is a net item; it is linked when an item without
/// raw id has that raw id as its own id. Of items with the same id the
/// last one counts.
///
/// Of several net items for one raw item only one is linked: the last
/// in the walk of the reference's hash table of the net items
/// ([`hash_walk`], ibx#599).
fn linked_net_ids(items: &[Value], class: ClassId, id: Slot, raw_id: Slot) -> Result<Vec<Option<String>>, Refused> {
    let mut plain: HashSet<Option<&str>> = HashSet::new();
    let mut net: HashMap<Option<&str>, &str> = HashMap::new();
    let mut net_ids: Vec<Option<&str>> = Vec::new();
    for item in items.iter().filter_map(node).filter(|n| is_a(n, class)) {
        match text(item, raw_id) {
            None | Some("") => {
                plain.insert(text(item, id));
            }
            Some(raw) => {
                if net.insert(text(item, id), raw).is_none() {
                    net_ids.push(text(item, id));
                }
            }
        }
    }
    let mut linked: HashMap<&str, Option<&str>> = HashMap::new();
    for net_id in hash_walk(&net_ids)? {
        let raw = net[&net_id];
        if plain.contains(&Some(raw)) {
            linked.insert(raw, net_id);
        }
    }
    Ok(linked.into_values().map(|net_id| net_id.map(str::to_string)).collect())
}

/// The order in which the reference walks a hash table of texts made
/// empty and filled with these keys, in this order, each once (ibx#599).
///
/// A key goes to the bucket of its hash spread over the table's size,
/// after the keys already there; the walk takes the buckets in order.
/// The table starts with 16 buckets and doubles when it holds more than
/// three quarters of that, or when a bucket reaches nine keys while it
/// has fewer than 64 buckets. A bucket of nine keys in a larger table
/// becomes a tree, whose walk is not modelled: refused. An item without
/// id is a key of hash 0.
fn hash_walk<'a>(keys: &[Option<&'a str>]) -> Result<Vec<Option<&'a str>>, Refused> {
    let hashes: Vec<u32> = keys.iter().map(|key| key.map_or(0, java_hash)).collect();
    let mut size: u32 = 16;
    for (i, &hash) in hashes.iter().enumerate() {
        let bucket = |h: u32, size: u32| h & (size - 1);
        let before = hashes[..i].iter().filter(|&&h| bucket(h, size) == bucket(hash, size)).count();
        if before >= 8 {
            if size >= 64 {
                return Err(Refused);
            }
            size *= 2;
        }
        if i + 1 > (size as usize) * 3 / 4 {
            size *= 2;
        }
    }
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| (hashes[i] & (size - 1), i));
    Ok(order.into_iter().map(|i| keys[i]).collect())
}

/// The hash of a text in the reference's tables: the text's hash over
/// its UTF-16 units, its high half folded into the low half.
fn java_hash(text: &str) -> u32 {
    let h = text.encode_utf16().fold(0u32, |h, unit| h.wrapping_mul(31).wrapping_add(u32::from(unit)));
    h ^ (h >> 16)
}

/// Net pricing (ibx#571): a client gets the raw items only. A scan type
/// with a `rawScanCode` leaves when a scan type without one has that
/// code as its `scanCode`; a filter of the filter list with a `rawId`
/// leaves when a filter without one has that id. The lists are those the
/// vendor rule left, so a net item whose raw item is gone stays. When
/// something left, the view of a client is taken once more.
fn drop_net_pricing_items(root: &mut Node, this_year: i32) -> Result<(), Refused> {
    let scan_types = child(root, RESPONSE_SCAN_TYPE_LIST).ok_or(Refused)?;
    let net_types = linked_net_ids(&scan_types.items, SCAN_TYPE, SCAN_TYPE_SCAN_CODE, SCAN_TYPE_RAW_SCAN_CODE)?;
    let no_filters = Vec::new();
    let filters = child(root, RESPONSE_FILTER_LIST);
    let net_filters = linked_net_ids(filters.map_or(&no_filters, |f| &f.items), ABSTRACT_FILTER, FILTER_ID, FILTER_RAW_ID)?;
    // The reference fails without a filter list.
    if filters.is_none() {
        return Err(Refused);
    }
    let is_in = |ids: &[Option<String>], id: Option<&str>| ids.iter().any(|linked| linked.as_deref() == id);

    let mut removed = false;
    if let Some(scan_types) = child_mut(root, RESPONSE_SCAN_TYPE_LIST) {
        let before = scan_types.items.len();
        scan_types.items.retain(|item| !node(item).is_some_and(|s| is_a(s, SCAN_TYPE) && is_in(&net_types, text(s, SCAN_TYPE_SCAN_CODE))));
        removed |= scan_types.items.len() != before;
    }
    if let Some(filters) = child_mut(root, RESPONSE_FILTER_LIST) {
        let before = filters.items.len();
        filters.items.retain(|item| !is_in(&net_filters, node(item).and_then(|f| text(f, FILTER_ID))));
        removed |= filters.items.len() != before;
    }
    if removed {
        client_view(root, this_year)?;
    }
    Ok(())
}
