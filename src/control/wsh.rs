//! The calendar data requests (WSH meta data and event data, ibx#443): the
//! request bodies, and the answers given to the client as the reference
//! gives them.
//!
//! The reference rebuilds what it receives before it gives it: the meta
//! data from its model of it, the events re-written. Its objects keep their
//! keys in a hash table, so a client receives them in the order of that
//! table, not in the server's; numbers are re-written (`1.50` gives `1.5`);
//! some characters are escaped (`€`), and every character above 128
//! that is left becomes `&#N;`. The request bodies follow the same order,
//! with the characters at or above 128 escaped as `\uXXXX`.

/// A JSON value, its object keys in the order they were read (no
/// duplicate: a key read again keeps its place, with the last value).
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A number, already in the form the reference writes it.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// The value of `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn obj(entries: Vec<(&str, Json)>) -> Json {
        Json::Obj(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// The text of a string, a number or a boolean, as an optional string
    /// of the reference reads it; `""` otherwise.
    fn text(&self) -> String {
        match self {
            Json::Str(s) | Json::Num(s) => s.clone(),
            Json::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    /// A boolean, from `true` or the text "true" (any case); false otherwise.
    fn truth(&self) -> bool {
        match self {
            Json::Bool(b) => *b,
            Json::Str(s) => s.eq_ignore_ascii_case("true"),
            _ => false,
        }
    }
}

// ── Reading ──

/// Read a JSON text the lenient way the reference reads one: single
/// quotes, keys and values without quotes, a trailing comma. `None` when
/// it is not JSON.
pub fn parse(text: &str) -> Option<Json> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let value = p.value()?;
    Some(value)
}

/// Read a JSON object; `None` when the text is not one.
pub fn parse_object(text: &str) -> Option<Json> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.skip();
    if p.peek() != Some(b'{') {
        return None;
    }
    p.value()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.skip();
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            q @ (b'"' | b'\'') => self.quoted(q).map(Json::Str),
            _ => {
                let start = self.i;
                while let Some(c) = self.peek() {
                    if c.is_ascii_whitespace() || b",:]}/\\\"[{;=#".contains(&c) {
                        break;
                    }
                    self.i += 1;
                }
                let token = std::str::from_utf8(&self.s[start..self.i]).ok()?.trim();
                if token.is_empty() {
                    return None;
                }
                Some(token_value(token))
            }
        }
    }

    fn quoted(&mut self, q: u8) -> Option<String> {
        self.i += 1;
        let mut out: Vec<u16> = Vec::new();
        let rest = std::str::from_utf8(&self.s[self.i..]).ok()?;
        let mut chars = rest.char_indices();
        while let Some((at, c)) = chars.next() {
            match c {
                c if c as u32 == q as u32 => {
                    self.i += at + 1;
                    return String::from_utf16(&out).ok();
                }
                '\n' | '\r' => return None,
                '\\' => {
                    let (_, e) = chars.next()?;
                    match e {
                        'b' => out.push(8),
                        't' => out.push(9),
                        'n' => out.push(10),
                        'f' => out.push(12),
                        'r' => out.push(13),
                        'u' => {
                            let hex: String = (0..4).filter_map(|_| chars.next().map(|(_, h)| h)).collect();
                            out.push(u16::from_str_radix(&hex, 16).ok()?);
                        }
                        '"' | '\'' | '\\' | '/' => out.push(e as u16),
                        _ => return None,
                    }
                }
                c => {
                    let mut units = [0u16; 2];
                    out.extend_from_slice(c.encode_utf16(&mut units));
                }
            }
        }
        None
    }

    fn array(&mut self) -> Option<Json> {
        self.i += 1;
        let mut items = Vec::new();
        self.skip();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Some(Json::Arr(items));
        }
        loop {
            self.skip();
            if self.peek() == Some(b',') {
                // An empty place reads as null.
                items.push(Json::Null);
            } else {
                items.push(self.value()?);
            }
            self.skip();
            match self.peek()? {
                b',' | b';' => {
                    self.i += 1;
                    self.skip();
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return Some(Json::Arr(items));
                    }
                }
                b']' => {
                    self.i += 1;
                    return Some(Json::Arr(items));
                }
                _ => return None,
            }
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.i += 1;
        let mut entries: Vec<(String, Json)> = Vec::new();
        loop {
            self.skip();
            match self.peek()? {
                b'}' => {
                    self.i += 1;
                    return Some(Json::Obj(entries));
                }
                _ => {
                    let key = match self.value()? {
                        Json::Str(s) | Json::Num(s) => s,
                        Json::Bool(b) => b.to_string(),
                        Json::Null => "null".to_string(),
                        _ => return None,
                    };
                    self.skip();
                    match self.peek()? {
                        b':' => self.i += 1,
                        b'=' => {
                            self.i += 1;
                            if self.peek() == Some(b'>') {
                                self.i += 1;
                            }
                        }
                        _ => return None,
                    }
                    let value = self.value()?;
                    match entries.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = value,
                        None => entries.push((key, value)),
                    }
                    self.skip();
                    match self.peek()? {
                        b',' | b';' => {
                            self.i += 1;
                            self.skip();
                            if self.peek() == Some(b'}') {
                                self.i += 1;
                                return Some(Json::Obj(entries));
                            }
                        }
                        b'}' => {
                            self.i += 1;
                            return Some(Json::Obj(entries));
                        }
                        _ => return None,
                    }
                }
            }
        }
    }
}

/// A token without quotes: a boolean, null, a number in the reference's
/// form, else a string.
fn token_value(token: &str) -> Json {
    match token.to_ascii_lowercase().as_str() {
        "true" => return Json::Bool(true),
        "false" => return Json::Bool(false),
        "null" => return Json::Null,
        _ => {}
    }
    number_text(token).map_or_else(|| Json::Str(token.to_string()), Json::Num)
}

/// A number as the reference writes it back: an integer as its digits, a
/// decimal or an exponent as a double in Java's form without trailing
/// zeros (`1.50` gives `1.5`, `1e3` gives `1000`). `None` when the token
/// is not a number.
pub fn number_text(token: &str) -> Option<String> {
    let first = token.as_bytes().first()?;
    if !(first.is_ascii_digit() || *first == b'-' || *first == b'.') {
        return None;
    }
    if !token.contains(['.', 'e', 'E']) && token != "-0" {
        let (sign, digits) = token.strip_prefix('-').map_or(("", token), |d| ("-", d));
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let digits = digits.trim_start_matches('0');
        return Some(if digits.is_empty() { "0".to_string() } else { format!("{sign}{digits}") });
    }
    let x: f64 = token.parse().ok()?;
    if !x.is_finite() {
        return None;
    }
    let text = java_double(x);
    Some(if text.contains('.') && !text.contains(['e', 'E']) {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    })
}

/// Java's `Double.toString`: decimal between 10^-3 and 10^7, else
/// `d.dddE±n`; the shortest digits that read back the same.
fn java_double(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let a = x.abs();
    if (1e-3..1e7).contains(&a) {
        let s = format!("{x}");
        return if s.contains('.') { s } else { format!("{s}.0") };
    }
    let s = format!("{x:e}");
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let mantissa = if mantissa.contains('.') { mantissa.to_string() } else { format!("{mantissa}.0") };
    format!("{mantissa}E{exp}")
}

// ── Writing ──

/// Java's `String.hashCode`.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(u as i32))
}

/// The order a Java hash table of default size gives `keys` (read in that
/// order): by bucket of the final table, then in the order read.
fn table_order<K: AsRef<str>>(keys: &[K]) -> Vec<usize> {
    let mut capacity = 16usize;
    while keys.len() * 4 > capacity * 3 {
        capacity *= 2;
    }
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| {
        let h = java_hash(keys[i].as_ref()) as u32;
        ((h ^ (h >> 16)) as usize) & (capacity - 1)
    });
    order
}

/// How the characters left after the JSON escapes are written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum High {
    /// To the client: every unit above 128 as `&#N;`.
    Entity,
    /// To the server: every unit at or above 128 as `\uXXXX`.
    Escape,
}

fn write(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(n),
        Json::Str(s) => quote(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Json::Obj(entries) => {
            out.push('{');
            let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
            for (n, i) in table_order(&keys).into_iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                quote(&entries[i].0, out);
                out.push(':');
                write(&entries[i].1, out);
            }
            out.push('}');
        }
    }
}

/// A string as the reference's JSON writer quotes it.
fn quote(s: &str, out: &mut String) {
    out.push('"');
    let mut prev = '\0';
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '/' if prev == '<' => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' || ('\u{80}'..'\u{a0}').contains(&c) || ('\u{2000}'..'\u{2100}').contains(&c) => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
        prev = c;
    }
    out.push('"');
}

fn finish(text: String, high: High) -> String {
    if text.is_ascii() {
        return text;
    }
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        if (c as u32) < 128 || (high == High::Entity && c as u32 == 128) {
            out.push(c);
            continue;
        }
        let mut units = [0u16; 2];
        for u in c.encode_utf16(&mut units) {
            match high {
                High::Entity => out.push_str(&format!("&#{};", u)),
                High::Escape => out.push_str(&format!("\\u{:04x}", u)),
            }
        }
    }
    out
}

/// A value written as the reference gives it to the client.
pub fn to_client(value: &Json) -> String {
    let mut out = String::new();
    write(value, &mut out);
    finish(out, High::Entity)
}

/// A value written as the reference sends it to the server.
pub fn to_server(value: &Json) -> String {
    let mut out = String::new();
    write(value, &mut out);
    finish(out, High::Escape)
}

// ── Requests ──

/// The capability the reference states in every request.
const CLIENT_CAPABILITY: &str = "Bw==";

/// The body of a meta data request.
pub fn meta_request() -> String {
    to_server(&Json::obj(vec![("calendar_request", Json::obj(vec![("client_capability", Json::str(CLIENT_CAPABILITY))]))]))
}

/// What an event data request asks, as the API gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRequest {
    /// The contract, `i32::MAX` when not given.
    pub con_id: i32,
    pub filter: String,
    pub fill_watchlist: bool,
    pub fill_portfolio: bool,
    pub fill_competitors: bool,
    pub start_date: String,
    pub end_date: String,
    /// `i32::MAX` when not given.
    pub total_limit: i32,
}

/// Why an event data request is not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRefusal {
    /// Neither or both of a contract and a filter: 10309.
    Invalid,
    /// A filter that is not a JSON object: 10283, with no reason.
    BadFilter,
    /// A start or end date not in `yyyyMMdd`: the reference only logs it,
    /// and the client gets nothing.
    BadDate,
}

/// Whether a date reads as `uuuuMMdd` with the reference's resolver: eight
/// digits, a month of 1 to 12, a day of 1 to 31 (a day past the month's
/// end is taken as its last day).
fn date_ok(date: &str) -> bool {
    date.len() == 8
        && date.bytes().all(|c| c.is_ascii_digit())
        && (1..=12).contains(&date[4..6].parse::<u32>().unwrap_or(0))
        && (1..=31).contains(&date[6..8].parse::<u32>().unwrap_or(0))
}

/// The body of an event data request, after the checks the reference makes
/// once the meta data is held: exactly one of a contract and a filter, a
/// filter that is a JSON object, readable dates.
pub fn event_request(r: &EventRequest) -> Result<String, EventRefusal> {
    let by_contract = r.con_id != i32::MAX;
    if by_contract == !r.filter.is_empty() {
        return Err(EventRefusal::Invalid);
    }
    let (filters, fills) = if by_contract {
        (Json::obj(vec![("watchlist", Json::Arr(vec![Json::str(r.con_id.to_string())]))]), (false, false, false))
    } else {
        let filters = parse_object(&r.filter).ok_or(EventRefusal::BadFilter)?;
        (filters, (r.fill_watchlist, r.fill_portfolio, r.fill_competitors))
    };
    let mut date = Vec::new();
    for (key, value) in [("start", &r.start_date), ("end", &r.end_date)] {
        if !value.is_empty() {
            if !date_ok(value) {
                return Err(EventRefusal::BadDate);
            }
            date.push((key, Json::str(value.clone())));
        }
    }
    let mut body = vec![
        ("sources", Json::Arr(vec![Json::str("WSHE")])),
        ("date", Json::obj(date)),
        ("account", Json::str("")),
        ("filters", filters),
        ("api", Json::Bool(true)),
        ("fill_watchlist", Json::Bool(fills.0)),
        ("fill_portfolio", Json::Bool(fills.1)),
        ("fill_competitors", Json::Bool(fills.2)),
        ("mode", Json::str("chronological")),
    ];
    if r.total_limit != i32::MAX {
        body.push(("total_limit", Json::str(r.total_limit.to_string())));
    }
    body.push(("client_capability", Json::str(CLIENT_CAPABILITY)));
    Ok(to_server(&Json::obj(vec![("calendar_request", Json::obj(body))])))
}

// ── Answers ──

/// The events of an event data answer, as the client gets them: the
/// `events` array of its calendar, re-written. `None` when the answer has
/// none (the reference's 10283 "Unable to parse JSON object").
pub fn events_answer(text: &str) -> Option<String> {
    let root = parse_object(text)?;
    let events = root.get("calendar")?.get("events")?;
    matches!(events, Json::Arr(_)).then(|| to_client(events))
}

/// The meta data the client gets: rebuilt from the answer as the
/// reference models it. Only the event types of the WSH sources, with a
/// fixed set of fields and the underlying's description column left out;
/// the filters without those of the WSH sources and of two other sources,
/// the IPO filter renamed. What cannot be read ends the reading, what was
/// read stays; an answer that cannot be read at all gives empty lists.
pub fn meta_answer(text: &str) -> String {
    let mut event_types = Vec::new();
    let mut filters = Vec::new();
    let _ = read_meta(text, &mut event_types, &mut filters);
    to_client(&Json::obj(vec![(
        "meta_data",
        Json::obj(vec![("event_types", Json::Arr(event_types)), ("filters", Json::Arr(filters))]),
    )]))
}

fn read_meta(text: &str, event_types: &mut Vec<Json>, filters: &mut Vec<Json>) -> Option<()> {
    let meta = parse_object(text)?;
    let meta = meta.get("meta_data")?;
    let Json::Arr(types) = meta.get("event_types")? else { return None };
    for t in types {
        let tag = t.get("tag").map(Json::text).unwrap_or_default();
        let Json::Arr(columns) = t.get("columns")? else { return None };
        if !tag.starts_with("wsh") {
            continue;
        }
        let s = |key: &str| Json::str(t.get(key).map(Json::text).unwrap_or_default());
        let group = t.get("group").map(Json::text).and_then(|g| g.parse::<i32>().ok()).unwrap_or(i32::MAX);
        let dates = match t.get("extra_index_dates") {
            Some(Json::Arr(d)) => Json::Arr(d.iter().map(|v| Json::str(v.text())).collect()),
            _ => Json::Arr(Vec::new()),
        };
        let columns = columns.iter()
            .filter(|c| c.get("tag").map(Json::text).as_deref() != Some("underlying_description"))
            .map(|c| {
                let s = |key: &str| Json::str(c.get(key).map(Json::text).unwrap_or_default());
                let b = |key: &str| Json::str(c.get(key).is_some_and(Json::truth).to_string());
                Json::obj(vec![
                    ("name", s("name")), ("tag", s("tag")), ("type", s("type")), ("group", s("group")),
                    ("alignment", s("alignment")), ("width", s("width")), ("format", s("format")),
                    ("scope", s("scope")), ("details_name", s("details_name")), ("view_mode", b("view_mode")),
                    ("primary", b("primary")), ("description", b("description")), ("hot_column", b("hot_column")),
                    ("event_details_only", b("event_details_only")), ("section", b("section")), ("index", b("index")),
                ])
            })
            .collect();
        event_types.push(Json::obj(vec![
            ("type", s("type")), ("tag", Json::str(tag)), ("name", s("name")), ("highlight", s("highlight")),
            ("group", Json::Num(group.to_string())), ("extra_index_dates", dates), ("columns", Json::Arr(columns)),
        ]));
    }
    let Json::Arr(list) = meta.get("filters")? else { return None };
    for f in list {
        let tag = f.get("tag").map(Json::text).unwrap_or_default();
        if tag.starts_with("wsh") || tag == "econoday_events" || tag == "rensc_event" {
            continue;
        }
        let kind = f.get("type").map(Json::text).unwrap_or_default();
        let s = |key: &str| Json::str(f.get(key).map(Json::text).unwrap_or_default());
        let b = |key: &str| Json::Bool(f.get(key).is_some_and(Json::truth));
        let value = match kind.as_str() {
            "combo" => match f.get("value") {
                Some(Json::Arr(v)) => Json::Arr(v.iter().map(|x| Json::str(x.text())).collect()),
                _ => Json::Arr(Vec::new()),
            },
            "boolean" | "boolean_conids" | "boolean_parent" => b("value"),
            _ => s("value"),
        };
        let exclude = match f.get("exclude") {
            Some(Json::Arr(v)) => Json::Arr(v.iter().map(|x| Json::str(x.text())).collect()),
            _ => Json::Arr(Vec::new()),
        };
        let name = if tag == "ipo" { Json::str("IPO/Secondary Offering") } else { s("name") };
        filters.push(Json::obj(vec![
            ("type", Json::str(kind)), ("tag", Json::str(tag)), ("name", name), ("value", value),
            ("parent", s("parent")), ("tooltip", s("tooltip")), ("header", b("header")),
            ("exclude", exclude), ("show_all", b("show_all")),
        ]));
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The reference's request bodies (gateway 1051's own writer).
    #[test]
    fn request_bodies_as_the_reference_writes_them() {
        assert_eq!(meta_request(), r#"{"calendar_request":{"client_capability":"Bw=="}}"#);
        let by_contract = |start: &str, end: &str, limit: i32| EventRequest {
            con_id: 8314, filter: String::new(), fill_watchlist: true, fill_portfolio: true, fill_competitors: true,
            start_date: start.into(), end_date: end.into(), total_limit: limit,
        };
        assert_eq!(event_request(&by_contract("", "", i32::MAX)).unwrap(),
            r#"{"calendar_request":{"date":{},"mode":"chronological","client_capability":"Bw==","sources":["WSHE"],"filters":{"watchlist":["8314"]},"api":true,"fill_watchlist":false,"fill_portfolio":false,"fill_competitors":false,"account":""}}"#);
        assert_eq!(event_request(&by_contract("20240101", "20241231", 5)).unwrap(),
            r#"{"calendar_request":{"date":{"start":"20240101","end":"20241231"},"mode":"chronological","client_capability":"Bw==","sources":["WSHE"],"filters":{"watchlist":["8314"]},"api":true,"fill_watchlist":false,"fill_portfolio":false,"fill_competitors":false,"account":"","total_limit":"5"}}"#);
        let by_filter = EventRequest {
            con_id: i32::MAX, filter: r#"{"country":"All","watchlist":["8314"],"limit_region":10,"limit":10}"#.into(),
            fill_watchlist: true, fill_portfolio: false, fill_competitors: true,
            start_date: "20240101".into(), end_date: String::new(), total_limit: i32::MAX,
        };
        assert_eq!(event_request(&by_filter).unwrap(),
            r#"{"calendar_request":{"date":{"start":"20240101"},"mode":"chronological","client_capability":"Bw==","sources":["WSHE"],"filters":{"country":"All","limit":10,"watchlist":["8314"],"limit_region":10},"api":true,"fill_watchlist":true,"fill_portfolio":false,"fill_competitors":true,"account":""}}"#);
        let mixed = EventRequest {
            filter: r#"{"z":1.50,"y":1e3,"x":null,"w":true,"v":"a/b</c","u":"é€","t":{"b":[1,2],"a":"é"},"s":12345678901}"#.into(),
            end_date: "20241231".into(), start_date: String::new(), total_limit: 0, ..by_filter.clone()
        };
        let body = event_request(&mixed).unwrap();
        assert!(body.contains(r#""filters":{"s":12345678901,"t":{"a":"\u00e9","b":[1,2]},"u":"\u00e9\u20ac","v":"a/b<\/c","w":true,"x":null,"y":1000,"z":1.5}"#), "{body}");
        assert!(body.contains(r#""date":{"end":"20241231"}"#) && body.ends_with(r#""total_limit":"0"}}"#), "{body}");
    }

    #[test]
    fn refused_event_requests() {
        let r = |con_id: i32, filter: &str, start: &str| event_request(&EventRequest {
            con_id, filter: filter.into(), fill_watchlist: false, fill_portfolio: false, fill_competitors: false,
            start_date: start.into(), end_date: String::new(), total_limit: i32::MAX,
        });
        assert_eq!(r(i32::MAX, "", ""), Err(EventRefusal::Invalid));
        assert_eq!(r(8314, "{}", ""), Err(EventRefusal::Invalid));
        assert_eq!(r(i32::MAX, "not json", ""), Err(EventRefusal::BadFilter));
        assert_eq!(r(i32::MAX, " ", ""), Err(EventRefusal::BadFilter));
        assert_eq!(r(i32::MAX, "[1]", ""), Err(EventRefusal::BadFilter));
        assert!(r(8314, "", "20240230").is_ok(), "a day past the month's end is read");
        for bad in ["2024-01-01", "202401011", "2024011", "20240001", " 20240101", "20240132"] {
            assert_eq!(r(8314, "", bad), Err(EventRefusal::BadDate), "{bad}");
        }
        assert!(r(i32::MAX, "{country:'All',}", "").is_ok(), "a lenient filter");
    }

    // The events as the reference gives them (gateway 1051's own reader
    // and writer, on made-up answers).
    #[test]
    fn events_as_the_reference_gives_them() {
        let answer = r#"{"calendar":{"events":[{"event_key":"k1","event_type":"wshe_ed","conids":["8314"],"source":"WSHE","filterSource":"watchlist","status":"Confirmed","country":"United States","index_date_type":"DATE","index_date":"20240425","data":{"time_of_day":"AMC","fiscal_quarter":2,"eps":1.50,"name":"Société €","url":"http://a/b"},"tooltips":{"x":"y"},"extra":123}],"has_more":true,"day_counts":[{"date":"20240425","count":{"watchlist":1,"portfolio":0}}]}}"#;
        assert_eq!(events_answer(answer).unwrap(),
            r#"[{"country":"United States","index_date_type":"DATE","event_type":"wshe_ed","data":{"time_of_day":"AMC","name":"Soci&#233;t&#233; \u20ac","eps":1.5,"fiscal_quarter":2,"url":"http://a/b"},"filterSource":"watchlist","extra":123,"conids":["8314"],"index_date":"20240425","source":"WSHE","event_key":"k1","tooltips":{"x":"y"},"status":"Confirmed"}]"#);
        assert_eq!(events_answer(r#"{"calendar":{"events":[],"has_more":false}}"#).unwrap(), "[]");
        assert_eq!(events_answer(r#"{"calendar":{"events":[{"event_key":"k1","b":1,"a":2}]}}"#).unwrap(), r#"[{"a":2,"b":1,"event_key":"k1"}]"#);
        assert_eq!(events_answer(r#"{"calendar":{"events":[{"conid":8314,"index_date_type":"DATE","index_date":"20240425","data":{}}]}}"#).unwrap(),
            r#"[{"index_date_type":"DATE","data":{},"conid":8314,"index_date":"20240425"}]"#);
        for none in [r#"{"calendar":{"has_more":false}}"#, r#"{"x":1}"#, "not json", ""] {
            assert_eq!(events_answer(none), None, "{none}");
        }
    }

    #[test]
    fn meta_data_as_the_reference_rebuilds_it() {
        let answer = concat!(r#"{"meta_data":{"event_types":["#,
            r#"{"type":"wshe_ed","tag":"wshe_ed","name":"Earnings Date","highlight":"h","group":3,"extra_index_dates":["a"],"zz":"kept?","columns":[{"name":"Time","tag":"time_of_day","type":"string","primary":true,"qq":1},{"name":"Und","tag":"underlying_description","type":"string"}]},"#,
            r#"{"type":"econoday","tag":"econoday_events","name":"Econ","columns":[]},"#,
            r#"{"type":"ipo","tag":"ipo","name":"IPO","columns":[]}],"#,
            r#""filters":["#,
            r#"{"type":"combo","tag":"country","name":"Country","value":["All","United States","Côte d'Ivoire"],"tooltip":"t","zz":1},"#,
            r#"{"type":"boolean","tag":"wshe_ed","name":"Earnings Dates","value":true,"parent":"p"},"#,
            r#"{"type":"boolean","tag":"econoday_events","name":"Econ","value":false},"#,
            r#"{"type":"boolean","tag":"rensc_event","name":"Ren","value":false},"#,
            r#"{"type":"boolean","tag":"ipo","name":"IPO","value":false},"#,
            r#"{"type":"string","tag":"limit","name":"Limit","value":"10"},"#,
            r#"{"type":"boolean_conids","tag":"watchlist","name":"Watchlist","value":false}]}}"#);
        assert_eq!(meta_answer(answer), concat!(
            r#"{"meta_data":{"event_types":[{"highlight":"h","extra_index_dates":["a"],"columns":[{"event_details_only":"false","view_mode":"false","format":"","description":"false","index":"false","section":"false","type":"string","hot_column":"false","details_name":"","scope":"","name":"Time","width":"","tag":"time_of_day","alignment":"","group":"","primary":"true"}],"name":"Earnings Date","tag":"wshe_ed","type":"wshe_ed","group":3}],"#,
            r#""filters":[{"parent":"","name":"Country","tooltip":"t","header":false,"exclude":[],"tag":"country","type":"combo","value":["All","United States","C&#244;te d'Ivoire"],"show_all":false},{"parent":"","name":"IPO/Secondary Offering","tooltip":"","header":false,"exclude":[],"tag":"ipo","type":"boolean","value":false,"show_all":false},{"parent":"","name":"Limit","tooltip":"","header":false,"exclude":[],"tag":"limit","type":"string","value":"10","show_all":false},{"parent":"","name":"Watchlist","tooltip":"","header":false,"exclude":[],"tag":"watchlist","type":"boolean_conids","value":false,"show_all":false}]}}"#));
        let empty = r#"{"meta_data":{"event_types":[],"filters":[]}}"#;
        for answer in [r#"{"meta_data":{"event_types":[]}}"#, r#"{"meta_data":{"filters":[]}}"#, "nope", "", empty] {
            assert_eq!(meta_answer(answer), empty, "{answer}");
        }
        // An event type without columns ends the reading: what came before stays.
        assert_eq!(meta_answer(r#"{"meta_data":{"event_types":[{"tag":"wshe_a","columns":[]},{"tag":"wshe_b"},{"tag":"wshe_c","columns":[]}],"filters":[{"type":"boolean","tag":"b","name":"B","value":true}]}}"#),
            r#"{"meta_data":{"event_types":[{"highlight":"","extra_index_dates":[],"columns":[],"name":"","tag":"wshe_a","type":"","group":2147483647}],"filters":[]}}"#);
        let full = concat!(r#"{"meta_data":{"event_types":[{"tag":"wshe_min","columns":[]},"#,
            r#"{"type":"t","tag":"wshe_full","name":"N","highlight":"yes","group":"7","columns":[{"name":"C","tag":"c1","group":"g","type":"date","alignment":"left","width":"40","format":"f","scope":"s","details_name":"d","view_mode":true,"primary":true,"description":true,"hot_column":true,"event_details_only":true,"section":true,"index":true}]}],"#,
            r#""filters":[{"type":"boolean_parent","tag":"bp","name":"BP","value":true,"header":true,"show_all":true,"exclude":["x","y"],"parent":"pp","tooltip":"tt"},"#,
            r#"{"type":"combo","tag":"limit_region","name":"LR","value":["10","20"]},{"type":"other","tag":"o","name":"O","value":123},{"type":"boolean","tag":"b","name":"B","value":"true"}]}}"#);
        assert_eq!(meta_answer(full), concat!(
            r#"{"meta_data":{"event_types":[{"highlight":"","extra_index_dates":[],"columns":[],"name":"","tag":"wshe_min","type":"","group":2147483647},{"highlight":"yes","extra_index_dates":[],"columns":[{"event_details_only":"true","view_mode":"true","format":"f","description":"true","index":"true","section":"true","type":"date","hot_column":"true","details_name":"d","scope":"s","name":"C","width":"40","tag":"c1","alignment":"left","group":"g","primary":"true"}],"name":"N","tag":"wshe_full","type":"t","group":7}],"#,
            r#""filters":[{"parent":"pp","name":"BP","tooltip":"tt","header":true,"exclude":["x","y"],"tag":"bp","type":"boolean_parent","value":true,"show_all":true},{"parent":"","name":"LR","tooltip":"","header":false,"exclude":[],"tag":"limit_region","type":"combo","value":["10","20"],"show_all":false},{"parent":"","name":"O","tooltip":"","header":false,"exclude":[],"tag":"o","type":"other","value":"123","show_all":false},{"parent":"","name":"B","tooltip":"","header":false,"exclude":[],"tag":"b","type":"boolean","value":true,"show_all":false}]}}"#));
    }

    #[test]
    fn numbers_in_the_reference_form() {
        for (raw, text) in [("1.50", "1.5"), ("1e3", "1000"), ("12345678901", "12345678901"), ("2", "2"), ("-3.25", "-3.25"),
            ("1e20", "1.0E20"), ("0.0001", "1.0E-4"), ("1.0", "1"), ("-0", "-0")] {
            assert_eq!(number_text(raw).as_deref(), Some(text), "{raw}");
        }
    }
}
