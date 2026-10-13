//! The scanner parameters of a client against texts the reference
//! produced (ibx#571): the document of a real session with what that
//! session had, variations of both, and small documents made for one
//! rule each.

use super::tests::{assert_same, fixture};
use super::*;

/// What the session of the real document had.
const OBSERVED: &str = "MOODY,SP,REUTFUND,WSH,MSOWN";

fn session(names: &str) -> Session {
    Session::new(names, [])
}

// ── The session ──

#[test]
fn session_names_are_the_features_cut_at_the_colon_and_the_news_provider_codes() {
    let s = Session::new("SECDEFTA,MOODY:2:x,,WSH:,:lost, SP", ["BRFG", "", "DJ-N"]);
    for name in ["SECDEFTA", "MOODY", "WSH", "BRFG", "DJ-N", " SP"] {
        assert!(s.has(name), "{name}");
    }
    for name in ["MOODY:2:x", "MOODY:2", "SP", "", "lost", "secdefta"] {
        assert!(!s.has(name), "{name}");
    }
    assert!(!s.no_research());
    assert_eq!(Session::new("", []), Session::default());
}

#[test]
fn session_switch_is_a_feature_of_the_list() {
    assert!(Session::new("A,NORSRCH", []).no_research());
    assert!(Session::new("NORSRCH:1", []).no_research());
    assert!(!Session::new("XNORSRCH,NORSRCHX,norsrch", []).no_research());
    // A news provider of that code is a name, not the switch.
    let s = Session::new("A", ["NORSRCH"]);
    assert!(s.has("NORSRCH") && !s.no_research());
}

// ── The document of a real session and its variations ──

#[test]
fn real_document_for_a_client_is_the_text_of_the_reference() {
    let got = for_client(&fixture("server.xml"), &session(OBSERVED)).expect("refused");
    let want = fixture("client.xml");
    assert_eq!(want.len(), 1_723_217);
    assert_same(&got, &want, "real document");
}

fn sub1(text: &str, old: &str, new: &str) -> String {
    assert!(text.contains(old), "{old}");
    text.replacen(old, new, 1)
}

/// Without the lines that hold only this element.
fn without_lines_of(text: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>\n"));
    let is_one = |line: &str| {
        let inner = line.trim_start_matches('\t').strip_prefix(&open).and_then(|rest| rest.strip_suffix(&close));
        inner.is_some_and(|inner| !inner.contains('<'))
    };
    text.split_inclusive('\n').filter(|line| !is_one(line)).collect()
}

/// The variations of the real document the reference was run on: name,
/// document, names of the session. The expected text of each is the
/// fixture `client_<name>.xml`; the one without fixture is refused.
fn probes(t: &str) -> Vec<(&'static str, String, &'static str)> {
    let mut out = vec![
        // Other sessions on the same document.
        ("v_none", t.to_string(), ""),
        ("v_all", t.to_string(), "ALV,MOODY,SP,REUTFUND,WSH,MSOWN,RCG"),
        ("v_alv_rcg", t.to_string(), "ALV,RCG"),
    ];
    let mut add = |name, text: String| out.push((name, text, OBSERVED));
    // An instrument with a context, and with a blank one.
    add("p1_context", sub1(t, "<name>US Stocks</name>", "<name>US Stocks</name>\n\t\t\t<context>MosaicScanner</context>"));
    add("p1_context_blank", sub1(t, "<name>US Stocks</name>", "<name>US Stocks</name>\n\t\t\t<context> </context>"));
    // A list that is not the country list: refused.
    add("p5_other_list", sub1(t, "<typeAheadList>iso2</typeAheadList>", "<typeAheadList>other</typeAheadList>"));
    // The texts that mean yes, and two that do not.
    let yes = "<nscanOnly>yes</nscanOnly>";
    let a = t.replacen(yes, "<nscanOnly>TRUE</nscanOnly>", 5).replacen(yes, "<nscanOnly>1</nscanOnly>", 5);
    let a = a.replacen(yes, "<nscanOnly>y</nscanOnly>", 5).replacen(yes, "<nscanOnly> yes</nscanOnly>", 5);
    add("p3_yes_values", a);
    // Groups: one nobody uses, one whose instruments are gone, another
    // order of first use.
    let a = sub1(t, "<group>STK.GLOBAL</group>\n\t\t</Instrument>", "<group>SSF.GLOBAL</group>\n\t\t</Instrument>");
    add("a2_groups", a.replace("<group>IND.GLOBAL</group>\n\t\t</Instrument>", "<group>NOPE</group>\n\t\t</Instrument>"));
    let (start, end) = (t.find("\t<InstrumentGroupList>").unwrap(), "</InstrumentGroupList>\n");
    let stop = start + t[start..].find(end).unwrap() + end.len();
    add("a2_no_groups", format!("{}{}", &t[..start], &t[stop..]));
    // A net scan type and a net filter whose raw item is missing stay.
    let a = sub1(t, "<scanCode>LOW_BOND_BID_YIELD_ALL</scanCode>", "<scanCode>X_LOW_BOND_BID_YIELD_ALL</scanCode>");
    add("a3_bad_links", sub1(&a, "<id>BOND_BID</id>", "<id>X_BOND_BID</id>"));
    // Nothing linked at all.
    add("a3_no_links", without_lines_of(&without_lines_of(t, "rawScanCode"), "rawId"));
    // A net item whose raw item the vendor rule removes first.
    add(
        "a3_raw_dropped_by_vendor",
        sub1(t, "<scanCode>LOW_BOND_BID_YIELD_ALL</scanCode>", "<scanCode>LOW_BOND_BID_YIELD_ALL</scanCode>\n\t\t\t<vendor>ALV</vendor>"),
    );
    // A scan code with the restricted prefix; a blank vendor.
    let a = sub1(t, "<scanCode>TOP_PERC_GAIN</scanCode>", "<scanCode>RESIDENCE_?X</scanCode>");
    add("p3_s1_edges", sub1(&a, "<scanCode>TOP_PERC_LOSE</scanCode>", "<scanCode>TOP_PERC_LOSE</scanCode><vendor> </vendor>"));
    // Filters with `nscanOnly`, one of them the converted one.
    let a = sub1(t, "<id>ISSUER_COUNTRY_CODE</id>", "<id>ISSUER_COUNTRY_CODE</id><nscanOnly>yes</nscanOnly>");
    add("p4_filter_nscan", sub1(&a, "<id>BOND_BID</id>", "<id>BOND_BID</id><nscanOnly>True</nscanOnly>"));
    // The members of a converted field show only inside it.
    add("p5_tooltip", sub1(t, "<code>issuerCountryIs</code>", "<code>issuerCountryIs</code><tooltip>tip</tooltip>"));
    out
}

#[test]
fn probes_for_a_client_are_the_texts_of_the_reference() {
    let real = fixture("server.xml");
    let all = probes(&real);
    assert_eq!(all.len(), 15);
    for (name, text, names) in all {
        let got = for_client(&text, &session(names));
        if name == "p5_other_list" {
            assert!(got.is_none(), "{name}: the reference refuses it");
            continue;
        }
        assert_same(&got.unwrap_or_else(|| panic!("{name}: refused")), &fixture(&format!("client_{name}.xml")), name);
    }
}

#[test]
fn switch_removes_every_vendor_item_of_the_real_document() {
    // The same as a session without any name.
    let real = fixture("server.xml");
    let got = for_client(&real, &Session::new(&format!("{OBSERVED},ALV,RCG,NORSRCH"), ["MOODY"])).expect("refused");
    assert_same(&got, &fixture("client_v_none.xml"), "no research");
}

#[test]
fn news_provider_code_keeps_the_items_of_its_vendor() {
    let real = fixture("server.xml");
    let got = for_client(&real, &Session::new("MOODY:1,SP,REUTFUND", ["WSH", "MSOWN"])).expect("refused");
    assert_same(&got, &fixture("client.xml"), "names from both sources");
}

// ── One rule each ──

/// A small valid document.
struct Doc<'a> {
    instruments: &'a str,
    scans: &'a str,
    /// Items of the filter list; `None` leaves the list out.
    filters: Option<&'a str>,
    /// Items of the second filter list.
    ui_filters: &'a str,
    /// Content of the sidecar section; `None` leaves it out.
    sidecar: Option<&'a str>,
    /// Other sections.
    more: &'a str,
}

impl Default for Doc<'_> {
    fn default() -> Self {
        Doc { instruments: "<Instrument><name>I</name></Instrument>", scans: "<ScanType><scanCode>S</scanCode></ScanType>", filters: Some(""), ui_filters: "", sidecar: Some(""), more: "" }
    }
}

impl Doc<'_> {
    fn xml(&self) -> String {
        let mut xml = format!(
            "<ScanParameterResponse><InstrumentList>{}</InstrumentList><LocationTree><Location><locationCode>L</locationCode></Location></LocationTree><ScanTypeList>{}</ScanTypeList>",
            self.instruments, self.scans
        );
        if let Some(filters) = self.filters {
            xml += &format!("<FilterList varName=\"filterList\">{filters}</FilterList>");
        }
        xml += &format!("<FilterList varName=\"uiFilters\">{}</FilterList>", self.ui_filters);
        if let Some(sidecar) = self.sidecar {
            xml += &format!("<SidecarScannerDefaults>{sidecar}</SidecarScannerDefaults>");
        }
        xml + self.more + "</ScanParameterResponse>"
    }

    /// The text for a client of a session with these names.
    fn client(&self, names: &str) -> Option<String> {
        for_client(&self.xml(), &session(names))
    }

    fn text(&self) -> String {
        self.client("").expect("refused")
    }
}

/// A section of the root in a written text, by the member it belongs to.
fn section<'a>(out: &'a str, member: &str) -> &'a str {
    let mark = format!(" varName=\"{member}\"");
    let at = out.find(&mark).unwrap_or_else(|| panic!("no section {member}"));
    let start = out[..at].rfind("\n\t<").unwrap() + 1;
    let tag = &out[start + 2..at];
    let line_end = start + out[start..].find('\n').unwrap();
    if out[start..line_end].ends_with("/>") {
        return &out[start..=line_end];
    }
    let close = format!("\n\t</{tag}>\n");
    &out[start..line_end + out[line_end..].find(&close).unwrap() + close.len()]
}

fn count(text: &str, what: &str) -> usize {
    text.matches(what).count()
}

fn scan(inner: &str) -> String {
    format!("<ScanType>{inner}</ScanType>")
}

fn simple_filter(id: &str, inner: &str) -> String {
    format!("<SimpleFilter><id>{id}</id>{inner}</SimpleFilter>")
}

/// A field of a simple filter that names a type-ahead list.
fn listed_field(list: &str) -> String {
    format!("<AbstractField type=\"scanner.filter.StringField\" varName=\"field\"><code>c</code><tooltip>tip</tooltip><typeAheadList>{list}</typeAheadList></AbstractField>")
}

const COMBO: &str = "<AbstractField type=\"scanner.filter.ComboField\" varName=\"field\"><ComboValues><ComboValue><code>v</code></ComboValue></ComboValues></AbstractField>";

#[test]
fn small_document_comes_out_whole() {
    let doc = Doc::default();
    assert_eq!(doc.client("").as_deref(), rebuild(&doc.xml()).as_deref());
    assert!(doc.text().contains("\t<SidecarScannerDefaults varName=\"sidecarScannerDefaults\"/>\n"));
}

#[test]
fn document_without_a_main_list_is_refused() {
    for doc in [
        Doc { instruments: "", ..Default::default() },
        Doc { scans: "", ..Default::default() },
        // Every scan type gone by the vendor rule.
        Doc { scans: "<ScanType><scanCode>S</scanCode><vendor>V</vendor></ScanType>", ..Default::default() },
    ] {
        assert!(doc.client("").is_none(), "{}", doc.xml());
    }
    let no_locations = Doc::default().xml().replace("<Location><locationCode>L</locationCode></Location>", "");
    assert!(for_client(&no_locations, &session("")).is_none());
    assert!(for_client("<ScanParameterResponse/>", &session("")).is_none());
    assert!(for_client("<scanner>XML</scanner>", &session("")).is_none());
    assert!(for_client("not xml", &session("")).is_none());
}

#[test]
fn document_without_the_sidecar_section_or_the_filter_list_is_refused() {
    assert!(Doc { sidecar: None, ..Default::default() }.client("").is_none());
    assert!(Doc { filters: None, ..Default::default() }.client("").is_none());
}

#[test]
fn instrument_with_a_context_leaves_the_list_but_not_the_full_list() {
    let doc = Doc {
        instruments: "<Instrument><name>A</name></Instrument><Instrument><name>B</name><context>Any</context></Instrument><Instrument><name>C</name><context> </context></Instrument><Instrument><name>D</name><context></context></Instrument>",
        ..Default::default()
    };
    let out = doc.text();
    let names = |part: &str| ["A", "B", "C", "D"].map(|n| count(part, &format!("<name>{n}</name>")));
    // A blank is a context, an empty text is none.
    assert_eq!(names(section(&out, "instrumentList")), [1, 0, 0, 1]);
    assert_eq!(names(section(&out, "fullInstrumentList")), [1, 1, 1, 1]);
}

#[test]
fn scan_type_that_is_not_for_clients_leaves() {
    let scans: String = [
        ("K1", ""),
        ("N1", "<nscanOnly>yes</nscanOnly>"),
        ("N2", "<nscanOnly>YeS</nscanOnly>"),
        ("N3", "<nscanOnly>TRUE</nscanOnly>"),
        ("N4", "<nscanOnly>1</nscanOnly>"),
        ("K2", "<nscanOnly>y</nscanOnly>"),
        ("K3", "<nscanOnly> yes</nscanOnly>"),
        ("K4", "<nscanOnly>01</nscanOnly>"),
        ("N5", "<instruments>WATCHLIST</instruments>"),
        ("K5", "<instruments>WATCHLIST,STK</instruments>"),
        ("K6", "<instruments>watchlist</instruments>"),
        ("RESIDENCE_?N6", ""),
        ("K7_RESIDENCE_?", ""),
        ("RESIDENCE_K8", ""),
    ]
    .iter()
    .map(|(code, more)| scan(&format!("<scanCode>{code}</scanCode>{more}")))
    .collect();
    let out = Doc { scans: &scans, ..Default::default() }.text();
    let list = section(&out, "scanTypeList");
    assert_eq!(count(list, "<ScanType>"), 8, "{list}");
    assert_eq!(count(list, "<scanCode>K"), 7);
    assert_eq!(count(list, "<scanCode>RESIDENCE_K8"), 1);
}

#[test]
fn scan_type_without_code_is_refused_unless_it_leaves_first() {
    let with = |inner: &str| Doc { scans: &format!("{}{}", scan("<scanCode>S</scanCode>"), scan(inner)), ..Default::default() }.client("");
    assert!(with("<displayName>no code</displayName>").is_none());
    assert!(with("<nscanOnly>yes</nscanOnly>").is_some());
    assert!(with("<instruments>WATCHLIST</instruments>").is_some());
    // An empty code is a code.
    assert!(with("<scanCode></scanCode>").is_some());
}

#[test]
fn item_of_another_class_among_the_scan_types_is_refused() {
    let scans = format!("{}<ScanType type=\"scanner.Instrument\"><name>N</name></ScanType>", scan("<scanCode>S</scanCode>"));
    let doc = Doc { scans: &scans, ..Default::default() };
    assert!(rebuild(&doc.xml()).is_some());
    assert!(doc.client("").is_none());
}

#[test]
fn filter_that_is_not_for_clients_leaves_the_filter_list_only() {
    let filters = [simple_filter("A", COMBO), simple_filter("B", &format!("<nscanOnly>True</nscanOnly>{COMBO}")), "<RangeFilter><id>C</id><nscanOnly>1</nscanOnly></RangeFilter>".to_string()].concat();
    let out = Doc { filters: Some(&filters), ui_filters: &filters, ..Default::default() }.text();
    let ids = |part: &str| ["A", "B", "C"].map(|id| count(part, &format!("<id>{id}</id>")));
    assert_eq!(ids(section(&out, "filterList")), [1, 0, 0]);
    assert_eq!(ids(section(&out, "uiFilters")), [1, 1, 1]);
}

#[test]
fn field_with_the_country_list_becomes_a_combo_of_the_countries() {
    let filter = simple_filter("F", &listed_field("iso2"));
    let out = Doc { filters: Some(&filter), ui_filters: &filter, ..Default::default() }.text();
    let list = section(&out, "filterList");
    let t = "\t\t\t";
    // The combo holds the field; its own members are those of a fresh
    // one, so the field's code and tooltip show once, inside.
    let head = format!(
        "{t}<AbstractField type=\"scanner.filter.ComboField$ConvertedComboField\" varName=\"field\">\n{t}\t<dontAllowClearAll>false</dontAllowClearAll>\n{t}\t<ComboValues varName=\"values\">\n{t}\t\t<ComboValue>\n{t}\t\t\t<code></code>\n{t}\t\t\t<displayName>All</displayName>\n{t}\t\t\t<tooltip>All</tooltip>\n{t}\t\t\t<default>true</default>\n{t}\t\t\t<syntheticAll>false</syntheticAll>\n{t}\t\t</ComboValue>\n{t}\t\t<ComboValue>\n{t}\t\t\t<code>AF</code>\n{t}\t\t\t<displayName>Afghanistan</displayName>\n{t}\t\t\t<tooltip>Afghanistan</tooltip>\n{t}\t\t\t<default>false</default>\n"
    );
    assert!(list.contains(&head), "{}", &list[..list.len().min(1500)]);
    let tail = format!(
        "{t}\t\t\t<code>ZW</code>\n{t}\t\t\t<displayName>Zimbabwe</displayName>\n{t}\t\t\t<tooltip>Zimbabwe</tooltip>\n{t}\t\t\t<default>false</default>\n{t}\t\t\t<syntheticAll>false</syntheticAll>\n{t}\t\t</ComboValue>\n{t}\t</ComboValues>\n{t}\t<master>false</master>\n{t}\t<narrowField>false</narrowField>\n{t}\t<radioButtons>false</radioButtons>\n{t}\t<typeAhead>true</typeAhead>\n{t}\t<AbstractField type=\"scanner.filter.StringField\" varName=\"field\">\n{t}\t\t<code>c</code>\n"
    );
    assert!(list.contains(&tail), "{}", &list[list.len().saturating_sub(1500)..]);
    assert_eq!(count(list, "<ComboValue>"), 250);
    assert_eq!(count(list, "<default>true</default>"), 1);
    assert_eq!(count(list, "<tooltip>tip</tooltip>"), 1);
    assert_eq!(count(list, "<typeAheadList>iso2</typeAheadList>"), 1);
    // The second filter list keeps the field as it came.
    let ui = section(&out, "uiFilters");
    assert_eq!(count(ui, "ConvertedComboField"), 0);
    assert_eq!(count(ui, "<AbstractField type=\"scanner.filter.StringField\" varName=\"field\">"), 1);
}

#[test]
fn country_entries_are_in_the_order_of_the_names() {
    use super::session_tables::COUNTRIES;
    assert_eq!(COUNTRIES.len(), 249);
    let units = |name: &str| name.encode_utf16().collect::<Vec<u16>>();
    assert!(COUNTRIES.windows(2).all(|w| units(w[0].1) < units(w[1].1)));
}

#[test]
fn field_with_another_list_or_without_list_and_filter_without_field() {
    // Another list: the combo has no values and the reference fails.
    assert!(Doc { filters: Some(&simple_filter("F", &listed_field("other"))), ..Default::default() }.client("").is_none());
    // Also when the filter then leaves the list.
    let leaving = simple_filter("F", &format!("<nscanOnly>yes</nscanOnly>{}", listed_field("other")));
    assert!(Doc { filters: Some(&leaving), ..Default::default() }.client("").is_some());
    // No list, or an empty one: the field stays.
    for list in ["", "<typeAheadList></typeAheadList>"] {
        let field = format!("<AbstractField type=\"scanner.filter.StringField\" varName=\"field\"><code>c</code>{list}</AbstractField>");
        let out = Doc { filters: Some(&simple_filter("F", &field)), ..Default::default() }.text();
        assert_eq!(count(&out, "ConvertedComboField"), 0);
    }
    // A simple filter without field: the reference fails.
    assert!(Doc { filters: Some(&simple_filter("F", "")), ..Default::default() }.client("").is_none());
    // In the second list it is not looked at.
    assert!(Doc { ui_filters: &simple_filter("F", ""), ..Default::default() }.client("").is_some());
}

#[test]
fn combo_field_without_values_is_refused() {
    let bare = "<AbstractField type=\"scanner.filter.ComboField\" varName=\"field\"/>";
    assert!(Doc { filters: Some(&simple_filter("F", bare)), ..Default::default() }.client("").is_none());
    assert!(Doc { filters: Some(&simple_filter("F", COMBO)), ..Default::default() }.client("").is_some());
    assert!(Doc { ui_filters: &simple_filter("F", bare), ..Default::default() }.client("").is_some());
}

/// The codes, display names and default flags of the first combo of the
/// filter list in a text written for a client in `year`.
fn computed(value: &str, value1: &str, field: &str, year: i32) -> Option<Vec<(String, String, bool)>> {
    let config = format!("<ConfigItem varName=\"config\"><ConfigItem><name>other</name></ConfigItem><ConfigItem name=\"values_generator\" value=\"{value}\" value1=\"{value1}\"/></ConfigItem>");
    let doc = Doc { filters: Some(&simple_filter("F", &format!("{config}{field}"))), ..Default::default() };
    let out = for_client_in(&doc.xml(), &session(""), year)?;
    // The values of the combo; the same element also feeds another member.
    let list = section(&out, "filterList");
    let Some(start) = list.find("<ComboValues varName=\"values\"") else { return Some(Vec::new()) };
    let open_end = start + list[start..].find('>').unwrap();
    let list = if list[..open_end].ends_with('/') { "" } else { &list[open_end..open_end + list[open_end..].find("</ComboValues>").unwrap()] };
    let member = |entry: &str, tag: &str| entry.split(&format!("<{tag}>")).nth(1).map(|rest| rest[..rest.find('<').unwrap()].to_string());
    Some(
        list.split("<ComboValue>")
            .skip(1)
            .map(|e| (member(e, "code").unwrap_or_default(), member(e, "displayName").unwrap_or_default(), member(e, "default").as_deref() == Some("true")))
            .collect(),
    )
}

#[test]
fn computed_values_are_one_entry_per_year_up_to_last_year() {
    let got = computed("years;2020;3Y", "Y; Year/ Years", COMBO, 2026).expect("refused");
    let want: Vec<_> = (1..=6).rev().map(|n| (format!("{n}Y"), format!("{n} {}", if n > 1 { "Years" } else { "Year" }), n == 3)).collect();
    assert_eq!(got, want);
    // The year of the machine's clock decides.
    assert_eq!(computed("years;2020;3Y", "Y; Year/ Years", COMBO, 2021).unwrap(), [("1Y".into(), "1 Year".into(), false)]);
    // From this year or later: no entry at all.
    assert_eq!(computed("years;2026;3Y", "Y; Year/ Years", COMBO, 2026).unwrap(), []);
    // A second display text alone serves for one year and several.
    let got = computed("years;2023;1M", "M;mo", COMBO, 2026).unwrap();
    assert_eq!(got, [("3M".into(), "3mo".into(), false), ("2M".into(), "2mo".into(), false), ("1M".into(), "1mo".into(), true)]);
}

#[test]
fn computed_values_left_as_they_are_or_refused() {
    let first = vec![("v".to_string(), String::new(), false)];
    for (value, value1) in [("years", "3"), ("months;2020;3Y", "Y; Y/ Ys"), ("years;20x;3Y", "Y; Y/ Ys"), ("years;2020;3Y", "Y"), ("years;2020;3Y", "")] {
        assert_eq!(computed(value, value1, COMBO, 2026).as_ref(), Some(&first), "{value} {value1}");
    }
    // Only a combo field has values to compute.
    assert!(computed("years;2020;3Y", "Y; Y/ Ys", "<AbstractField type=\"scanner.filter.StringField\" varName=\"field\"/>", 2026).is_some());
    // A value cut into nothing, and a combo without values, fail the reference.
    assert!(computed(";", "Y; Y/ Ys", COMBO, 2026).is_none());
    assert!(computed("years;2020;3Y", "Y; Y/ Ys", "<AbstractField type=\"scanner.filter.ComboField\" varName=\"field\"/>", 2026).is_none());
    // A start year beyond what the reference's calendar holds.
    assert!(computed("years;2147483647;3Y", "Y; Y/ Ys", COMBO, 2026).is_none());
}

#[test]
fn computed_values_years_before_the_era_count_down_first() {
    let codes = |value: &str| computed(value, "Y; Y/ Ys", COMBO, 5).unwrap().into_iter().map(|e| e.0).collect::<Vec<_>>();
    assert_eq!(codes("years;1;x"), ["4Y", "3Y", "2Y", "1Y"]);
    assert_eq!(codes("years;0;x"), ["4Y", "4Y", "3Y", "2Y", "1Y"]);
    assert_eq!(codes("years;-2;x"), ["2Y", "3Y", "4Y", "4Y", "3Y", "2Y", "1Y"]);
    // So far back that the calendar shows a year past this one.
    assert_eq!(codes("years;-4;x"), Vec::<String>::new());
    // Digits of another script, and a sign.
    assert_eq!(codes("years;\u{FF13};x"), ["2Y", "1Y"]);
    assert_eq!(codes("years;+3;x"), ["2Y", "1Y"]);
    assert_eq!(codes("years;\u{0663}\u{0664};x").len(), 0);
}

#[test]
fn converted_combo_field_cannot_be_read_from_a_document() {
    let field = "<AbstractField type=\"scanner.filter.ComboField$ConvertedComboField\" varName=\"field\"/>";
    let doc = Doc { filters: Some(&simple_filter("F", field)), ..Default::default() };
    assert!(rebuild(&doc.xml()).is_none());
    assert!(doc.client("").is_none());
    assert!(Node::new("scanner.filter.ComboField$ConvertedComboField").is_none());
}

#[test]
fn vendor_items_stay_for_a_session_that_has_the_vendor() {
    let scans = [scan("<scanCode>S0</scanCode>"), scan("<scanCode>S1</scanCode><vendor>V1</vendor>"), scan("<scanCode>S2</scanCode><vendor>V2</vendor>"), scan("<scanCode>S3</scanCode><vendor> </vendor>"), scan("<scanCode>S4</scanCode><vendor></vendor>")].concat();
    let filters = [simple_filter("F0", COMBO), simple_filter("F1", &format!("<vendor>V1</vendor>{COMBO}")), "<RangeFilter><id>F2</id><vendor>V2</vendor></RangeFilter>".to_string()].concat();
    let template = |name: &str, vendor: &str| format!("<SidecarScannerTemplate><name>{name}</name>{vendor}</SidecarScannerTemplate>");
    let sidecar = format!("<SidecarScannerTemplateList>{}{}{}</SidecarScannerTemplateList>", template("T0", ""), template("T1", "<vendor>V1</vendor>"), template("T2", "<vendor>V2</vendor>"));
    let doc = Doc { scans: &scans, filters: Some(&filters), ui_filters: &filters, sidecar: Some(&sidecar), ..Default::default() };

    let kept = |session: &Session| {
        let out = for_client(&doc.xml(), session).expect("refused");
        let of = |part: &str, what: &[&str]| what.iter().map(|w| count(part, w).to_string()).collect::<String>();
        let codes = ["<scanCode>S0<", "<scanCode>S1<", "<scanCode>S2<", "<scanCode>S3<", "<scanCode>S4<"];
        let ids = ["<id>F0<", "<id>F1<", "<id>F2<"];
        let names = ["<name>T0<", "<name>T1<", "<name>T2<"];
        [of(section(&out, "scanTypeList"), &codes), of(section(&out, "filterList"), &ids), of(section(&out, "uiFilters"), &ids), of(section(&out, "sidecarScannerDefaults"), &names)].join(" ")
    };
    // No vendor and the empty vendor stay; a blank is a vendor nobody has.
    assert_eq!(kept(&Session::new("", [])), "10001 100 111 100");
    assert_eq!(kept(&Session::new("V1:7,OTHER", [])), "11001 110 111 110");
    assert_eq!(kept(&Session::new("OTHER", ["V2"])), "10101 101 111 101");
    assert_eq!(kept(&Session::new("V1", ["V2"])), "11101 111 111 111");
    assert_eq!(kept(&Session::new("V1,NORSRCH", ["V2"])), "10001 100 111 100");
    assert_eq!(kept(&Session::new("V1, ", [" "])), "11011 110 111 110");
}

#[test]
fn one_instrument_is_added_per_group_in_use() {
    let instrument = |name: &str, group: &str| format!("<Instrument><name>{name}</name>{group}</Instrument>");
    let group = |id: &str, more: &str| format!("<InstrumentGroup><group>{id}</group>{more}</InstrumentGroup>");
    let instruments = [instrument("A", "<group>G2</group>"), instrument("B", ""), instrument("C", "<group>G1</group>"), instrument("D", "<group>G2</group>"), instrument("E", "<group>NONE</group>")].concat();
    let groups = format!(
        "<InstrumentGroupList>{}{}{}{}</InstrumentGroupList>",
        group("G1", "<name>One</name><secType>STK</secType><nscanSecType>STOCK</nscanSecType><routeExchange>X</routeExchange>"),
        group("G2", "<name>Two</name><secType>FUT</secType><nscanSecType></nscanSecType>"),
        // The first group of a name counts; one nobody uses gives nothing.
        group("G2", "<name>Two again</name>"),
        group("G3", "<name>Three</name>"),
    );
    let out = Doc { instruments: &instruments, more: &groups, ..Default::default() }.text();
    let new = |name: &str, sec: &str, nscan: &str| {
        format!("\t\t<Instrument>\n\t\t\t<name>{name}</name>\n\t\t\t<type>Global</type>\n\t\t\t<secType>{sec}</secType>\n\t\t\t<nscanSecType>{nscan}</nscanSecType>\n\t\t\t<cloudScanNotSupported>false</cloudScanNotSupported>\n\t\t</Instrument>\n")
    };
    // In the order of first use, at the end of both lists.
    let end = format!("{}{}\t</InstrumentList>\n", new("Two", "FUT", "FUT"), new("One", "STK", "STOCK"));
    for member in ["instrumentList", "fullInstrumentList"] {
        let list = section(&out, member);
        assert!(list.ends_with(&end), "{list}");
        assert_eq!(count(list, "<Instrument>"), 7);
    }
    assert_eq!(count(&out, "Two again") + count(&out, "<name>Three</name>"), 2, "only in the group list");

    // A group without values gives an instrument without them.
    let bare = Doc { instruments: &instrument("A", "<group>G</group>"), more: "<InstrumentGroupList><InstrumentGroup><group>G</group></InstrumentGroup></InstrumentGroupList>", ..Default::default() }.text();
    assert!(section(&bare, "instrumentList").ends_with("\t\t<Instrument>\n\t\t\t<type>Global</type>\n\t\t\t<cloudScanNotSupported>false</cloudScanNotSupported>\n\t\t</Instrument>\n\t</InstrumentList>\n"), "{bare}");
}

#[test]
fn group_instruments_where_the_reference_fails_or_does_nothing() {
    let instrument = "<Instrument><name>A</name><group>G</group></Instrument>";
    let groups = "<InstrumentGroupList><InstrumentGroup><name>no id</name></InstrumentGroup><InstrumentGroup><group>G</group></InstrumentGroup></InstrumentGroupList>";
    // A group without `group` met while looking for one: refused.
    assert!(Doc { instruments: instrument, more: groups, ..Default::default() }.client("").is_none());
    // Not met when no instrument names a group.
    assert!(Doc { more: groups, ..Default::default() }.client("").is_some());
    // No group list, or an empty one: nothing is added.
    for more in ["", "<InstrumentGroupList/>"] {
        let out = Doc { instruments: instrument, more, ..Default::default() }.text();
        assert_eq!(count(&out, "<type>Global</type>"), 0);
    }
    // An instrument that left the list for its context does not count.
    let hidden = "<Instrument><name>A</name></Instrument><Instrument><name>B</name><group>G</group><context>x</context></Instrument>";
    let out = Doc { instruments: hidden, more: "<InstrumentGroupList><InstrumentGroup><group>G</group></InstrumentGroup></InstrumentGroupList>", ..Default::default() }.text();
    assert_eq!(count(&out, "<type>Global</type>"), 0);
    // Without a full list there is nowhere to add to: refused.
    let only_list = Doc { instruments: instrument, more: "<InstrumentGroupList><InstrumentGroup><group>G</group></InstrumentGroup></InstrumentGroupList>", ..Default::default() }
        .xml()
        .replace("<InstrumentList>", "<InstrumentList varName=\"instrumentList\">");
    assert!(rebuild(&only_list).is_some());
    assert!(for_client(&only_list, &session("")).is_none());
}

#[test]
fn net_item_with_a_raw_counterpart_leaves() {
    let scans = [
        scan("<scanCode>RAW</scanCode>"),
        scan("<scanCode>NET</scanCode><rawScanCode>RAW</rawScanCode>"),
        // Its raw item is missing, is itself a net item, or left before.
        scan("<scanCode>NET2</scanCode><rawScanCode>MISSING</rawScanCode>"),
        scan("<scanCode>NET3</scanCode><rawScanCode>NET</rawScanCode>"),
        scan("<scanCode>RAW4</scanCode><vendor>V</vendor>"),
        scan("<scanCode>NET4</scanCode><rawScanCode>RAW4</rawScanCode>"),
        scan("<scanCode>RAW5</scanCode><nscanOnly>yes</nscanOnly>"),
        scan("<scanCode>NET5</scanCode><rawScanCode>RAW5</rawScanCode>"),
        // An empty raw code is none.
        scan("<scanCode>PLAIN</scanCode><rawScanCode></rawScanCode>"),
    ]
    .concat();
    let filters = [
        simple_filter("RAW", COMBO),
        format!("<RangeFilter><id>NET</id><rawId>RAW</rawId></RangeFilter>"),
        simple_filter("NET2", &format!("<rawId>MISSING</rawId>{COMBO}")),
        // Converted when the view of a client is taken the first time,
        // not again when it is taken once more.
        simple_filter("LISTED", &listed_field("iso2")),
    ]
    .concat();
    let out = Doc { scans: &scans, filters: Some(&filters), ui_filters: &filters, ..Default::default() }.text();
    let list = section(&out, "scanTypeList");
    let codes: Vec<&str> = list.lines().filter_map(|l| l.trim().strip_prefix("<scanCode>")?.strip_suffix("</scanCode>")).collect();
    assert_eq!(codes, ["RAW", "NET2", "NET3", "NET4", "NET5", "PLAIN"]);
    let ids = |part: &str| ["RAW", "NET", "NET2", "LISTED"].map(|id| count(part, &format!("<id>{id}</id>")));
    assert_eq!(ids(section(&out, "filterList")), [1, 0, 1, 1]);
    assert_eq!(ids(section(&out, "uiFilters")), [1, 1, 1, 1]);
    assert_eq!(count(section(&out, "filterList"), "ConvertedComboField"), 1);
}

/// The scan codes left for a client when these net codes all name one
/// raw scan type.
fn kept_nets(nets: &[&str]) -> Vec<String> {
    let mut scans = scan("<scanCode>RAW</scanCode>");
    for code in nets {
        scans += &scan(&format!("<scanCode>{code}</scanCode><rawScanCode>RAW</rawScanCode>"));
    }
    let out = Doc { scans: &scans, ..Default::default() }.text();
    let list = section(&out, "scanTypeList");
    list.lines().filter_map(|l| l.trim().strip_prefix("<scanCode>")?.strip_suffix("</scanCode>")).filter(|c| *c != "RAW").map(str::to_string).collect()
}

#[test]
fn of_net_items_for_one_raw_item_the_last_of_the_hash_walk_leaves() {
    // Buckets of 16: "N1" 3, "N2" 4, "B" 2, "C" 3. The walk goes by
    // bucket, then in the order the keys were put.
    assert_eq!(kept_nets(&["N1", "N2"]), ["N1"]);
    assert_eq!(kept_nets(&["N2", "N1"]), ["N1"]);
    assert_eq!(kept_nets(&["N1", "B"]), ["B"]);
    assert_eq!(kept_nets(&["N1", "C"]), ["N1"]);
    assert_eq!(kept_nets(&["C", "N1"]), ["C"]);
    // The same net item twice is one.
    assert_eq!(kept_nets(&["N1", "N1"]), Vec::<String>::new());
    let filters = "<RangeFilter><id>RAW</id></RangeFilter><RangeFilter><id>N2</id><rawId>RAW</rawId></RangeFilter><RangeFilter><id>N1</id><rawId>RAW</rawId></RangeFilter>";
    let out = Doc { filters: Some(filters), ..Default::default() }.text();
    assert_eq!(["N1", "N2"].map(|id| count(section(&out, "filterList"), &format!("<id>{id}</id>"))), [1, 0]);
}

#[test]
fn hash_walk_grows_the_table_by_full_buckets() {
    // Ten keys of bucket 7 of 32, four of them in bucket 7 of 64 and six
    // in bucket 39: the table grows to 32 at the ninth key of the bucket
    // and to 64 at the tenth, although ten keys fit in 16. "N144", last
    // put, walks before those of bucket 39; "N177" is last.
    let names = ["N80", "N5", "N91", "N100", "N111", "N133", "N122", "N155", "N177", "N144"];
    let kept = kept_nets(&names);
    assert_eq!(kept.len(), 9);
    assert!(kept.contains(&"N144".to_string()) && !kept.contains(&"N177".to_string()), "{kept:?}");
}

/// A case of the reference's own code (#582 and #599): the server text
/// and the text written for a client (`None`: none), each kept as line
/// hunks over the real document.
fn oracle_cases() -> Vec<(String, String, Option<String>)> {
    let packed = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gw1040/scanner_params/oracle_599.json.gz")).unwrap();
    let mut json = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&packed[..]), &mut json).unwrap();
    let data: serde_json::Value = serde_json::from_str(&json).unwrap();
    let (server, client) = (fixture("server.xml"), fixture("client.xml"));
    let apply = |base: &str, hunks: &serde_json::Value| {
        let lines: Vec<&str> = base.split_inclusive('\n').collect();
        let (mut out, mut at) = (String::new(), 0);
        for hunk in hunks.as_array().unwrap() {
            let (first, end) = (hunk[0].as_u64().unwrap() as usize, hunk[1].as_u64().unwrap() as usize);
            out.extend(lines[at..first].iter().copied());
            out += hunk[2].as_str().unwrap();
            at = end;
        }
        out.extend(lines[at..].iter().copied());
        out
    };
    data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let output = (!c["output"].is_null()).then(|| apply(&client, &c["output"]));
            (c["name"].as_str().unwrap().to_string(), apply(&server, &c["input"]), output)
        })
        .collect()
}

#[test]
fn computed_values_and_net_items_are_the_texts_of_the_reference() {
    let cases = oracle_cases();
    assert_eq!(cases.len(), 73);
    for (name, input, want) in cases {
        // The reference's code ran in 2026.
        let got = for_client_in(&input, &session(OBSERVED), 2026);
        match (got, want) {
            (Some(got), Some(want)) => assert_same(&got, &want, &name),
            (None, None) => {}
            (got, want) => panic!("{name}: ibx gives a text: {}, the reference: {}", got.is_some(), want.is_some()),
        }
    }
}
