//! The rebuilt scanner parameters against texts the reference produced
//! (ibx#571): the document of a real session, variations of it, and
//! small documents made for one rule each. The inputs of `gw1040/` were
//! recorded with the reference 10.40; the expected texts are those the
//! reference 10.51 writes for the same inputs (ibx#716).

use std::io::Read;
use std::path::PathBuf;

use super::tables::{CLASSES, ENUMS, ROOT};
use super::*;

/// A fixture by name; the large ones are stored compressed.
pub(super) fn fixture(name: &str) -> String {
    fixture_of("gw1040", name)
}

/// A fixture of the recordings of one version of the reference.
pub(super) fn fixture_of(version: &str, name: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(version).join("scanner_params");
    if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
        return text;
    }
    let packed = std::fs::read(dir.join(format!("{name}.gz"))).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    let mut text = String::new();
    flate2::read::GzDecoder::new(&packed[..]).read_to_string(&mut text).unwrap();
    text
}

/// Compare two long texts without printing them whole.
pub(super) fn assert_same(got: &str, want: &str, what: &str) {
    if got == want {
        return;
    }
    let at = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
    let cut = |s: &str| {
        let lo = (at.saturating_sub(200)..=at).find(|&i| s.is_char_boundary(i)).unwrap_or(0);
        let hi = (at + 200).min(s.len());
        let hi = (hi..=s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
        s[lo..hi].to_string()
    };
    panic!("{what}: differs at byte {at} (got {} bytes, want {})\n--- got\n{}\n--- want\n{}", got.len(), want.len(), cut(got), cut(want));
}

fn doc(body: &str) -> String {
    format!("<ScanParameterResponse>{body}</ScanParameterResponse>")
}

/// The text written for a document, without the header line.
fn body_of(xml: &str) -> String {
    let out = rebuild(xml).expect("refused");
    out.strip_prefix("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n").expect("header").to_string()
}

fn items<'a>(model: &'a Model, list: &str) -> &'a [Value] {
    &model.root.child(list).unwrap_or_else(|| panic!("no {list}")).items
}

fn item<'a>(model: &'a Model, list: &str, i: usize) -> &'a Node {
    match &items(model, list)[i] {
        Value::Node(n) => n,
        other => panic!("item {i} of {list}: {other:?}"),
    }
}

// ── The generated tables ──

#[test]
fn tables_are_consistent() {
    assert_eq!(simple_name(CLASSES[ROOT as usize].name), "ScanParameterResponse");
    for (i, class) in CLASSES.iter().enumerate() {
        let me = i as ClassId;
        match class.kind {
            Kind::Entries | Kind::Silent => assert!(class.chain.is_empty() && class.members.is_empty(), "{}", class.name),
            _ => assert_eq!(class.chain.last(), Some(&me), "{}", class.name),
        }
        assert!(!(class.abs && class.make), "{}", class.name);
        let total: usize = class.chain.iter().map(|&l| CLASSES[l as usize].members.len()).sum();
        assert!(class.over.iter().all(|&(slot, _)| (slot as usize) < total), "{}", class.name);
        for member in class.members {
            match member.ty {
                Ty::Obj(c) => assert!((c as usize) < CLASSES.len()),
                Ty::Enum(e) => assert!((e as usize) < ENUMS.len()),
                _ => {}
            }
            // The default of a member is of the member's type.
            let fits = match (member.def, member.ty) {
                (Def::Null, _) => true,
                (Def::Str(_), Ty::Str) | (Def::Int(_), Ty::Int) | (Def::Bool(_), Ty::Bool) => true,
                (Def::Enum(name), Ty::Enum(e)) => ENUMS[e as usize].consts.contains(&name),
                (Def::Opaque, Ty::Obj(_) | Ty::Foreign(_)) => true,
                _ => false,
            };
            assert!(fits, "{}.{}", class.name, member.tag);
        }
        match class.kind {
            Kind::List(Items::Class(c)) => assert!((c as usize) < CLASSES.len()),
            Kind::Flex(classes) => assert!(classes.iter().all(|&c| (c as usize) < CLASSES.len())),
            _ => {}
        }
        if class.make {
            let node = Node::of(me).unwrap();
            assert_eq!(node.members.len(), total, "{}", class.name);
        } else {
            assert!(Node::of(me).is_none());
        }
    }
    assert!(ENUMS.iter().all(|e| !e.consts.is_empty()));
}

// ── The document of a real session and its variations ──

#[test]
fn real_document_is_rebuilt_as_the_reference_does() {
    let got = rebuild(&fixture("server.xml")).expect("refused");
    let want = fixture("rebuilt.xml");
    assert_eq!(want.len(), 2_082_365);
    assert_same(&got, &want, "real document");
}

/// Each probe with the text the reference wrote for it; `None` where
/// the reference wrote nothing.
const PROBES: &[(&str, Option<&str>)] = &[
    ("d01_drop_sections.xml", Some("d01_drop_sections.ref.xml")),
    ("d02_drop_optional_children.xml", Some("d02_drop_optional_children.ref.xml")),
    ("d03_reorder.xml", Some("d03_reorder.ref.xml")),
    // Unknown elements and attributes change nothing.
    ("d04_unknown_things.xml", Some("rebuilt.xml")),
    ("d05_reference_output_as_input.xml", Some("d05_reference_output_as_input.ref.xml")),
    ("d06_attributes_and_children_swapped.xml", Some("d06_attributes_and_children_swapped.ref.xml")),
    ("s01_scalars.xml", Some("s01_scalars.ref.xml")),
    ("s02_objects.xml", Some("s02_objects.ref.xml")),
    ("s03_bad_type_attribute.xml", Some("s03_bad_type_attribute.ref.xml")),
    ("s04_characters.xml", Some("s04_characters.ref.xml")),
    ("x01_abstract_member_unknown_tag.xml", None),
    ("x02_filter_entry_with_nested_element.xml", Some("x02_filter_entry_with_nested_element.ref.xml")),
    ("x03_native_type_attribute_on_object_member.xml", Some("x03_native_type_attribute_on_object_member.ref.xml")),
    ("x04_array_tag.xml", None),
    ("x05_not_well_formed.xml", None),
];

#[test]
fn probes_are_rebuilt_as_the_reference_does() {
    for &(input, expected) in PROBES {
        let got = rebuild(&fixture(input));
        match expected {
            Some(expected) => assert_same(&got.unwrap_or_else(|| panic!("{input}: refused")), &fixture(expected), input),
            None => assert!(got.is_none(), "{input}: the reference refuses it"),
        }
    }
}

#[test]
fn rebuilding_the_rebuilt_text_is_stable_from_the_second_pass() {
    // The first pass over the reference's own output still adds to it
    // (probe d05); after that nothing changes any more.
    let second = rebuild(&fixture("d05_reference_output_as_input.ref.xml")).expect("refused");
    assert_same(&rebuild(&second).expect("refused"), &second, "third pass");
}


// ── One rule each ──

#[test]
fn root_name_is_not_checked() {
    assert_eq!(body_of("<Anything/>"), "<ScanParameterResponse/>\n");
}

#[test]
fn one_element_feeds_every_member_of_its_class() {
    // Two members are instrument lists: an element without `varName`
    // fills both, so the section is written twice.
    let body = body_of(&doc("<InstrumentList><Instrument><name>N</name></Instrument></InstrumentList>"));
    let section = |var: &str| {
        format!(
            "\t<InstrumentList varName=\"{var}\">\n\t\t<Instrument>\n\t\t\t<name>N</name>\n\t\t\t<cloudScanNotSupported>false</cloudScanNotSupported>\n\t\t</Instrument>\n\t</InstrumentList>\n"
        )
    };
    assert_eq!(body, format!("<ScanParameterResponse>\n{}{}</ScanParameterResponse>\n", section("instrumentList"), section("fullInstrumentList")));

    // With a `varName` it belongs to that member only.
    let body = body_of(&doc("<InstrumentList varName=\"fullInstrumentList\"><Instrument><name>N</name></Instrument></InstrumentList>"));
    assert_eq!(body, format!("<ScanParameterResponse>\n{}</ScanParameterResponse>\n", section("fullInstrumentList")));
}

#[test]
fn scalar_is_read_from_the_child_element_then_from_the_attribute() {
    let model = parse(&doc(concat!(
        "<InstrumentList><Instrument name=\"attr\" shortName=\"attr only\" m_type=\"T\" type=\"ignored\">",
        "<name>child</name><group>a<!-- c -->b</group><secType><b>x</b></secType><filters>x<b>n</b>y</filters>",
        "</Instrument></InstrumentList>"
    )))
    .unwrap();
    let instrument = item(&model, "instrumentList", 0);
    assert_eq!(instrument.text("name"), Some("child"));
    assert_eq!(instrument.text("shortName"), Some("attr only"));
    // The attribute of the member `type` is `m_type`.
    assert_eq!(instrument.text("type"), Some("T"));
    // Several children: all character data, the comment included.
    assert_eq!(instrument.text("group"), Some("a c b"));
    assert_eq!(instrument.text("filters"), Some("xy"));
    // A lone child element gives no text: the member keeps its default.
    assert_eq!(instrument.get("secType"), Some(&Value::Null));
    // Absent everywhere: the default stays, and a scalar one is written.
    assert_eq!(instrument.get("cloudScanNotSupported"), Some(&Value::Bool(false)));
    assert_eq!(instrument.get("context"), Some(&Value::Null));
}

#[test]
fn blank_text_with_a_line_break_or_a_tab_is_read_as_empty() {
    let model = parse(&doc(
        "<InstrumentList><Instrument><name>\n\t</name><group>   </group><secType> a\n</secType><filters>&#9;</filters></Instrument></InstrumentList>",
    ))
    .unwrap();
    let instrument = item(&model, "instrumentList", 0);
    assert_eq!(instrument.text("name"), Some(""));
    assert_eq!(instrument.text("filters"), Some(""));
    assert_eq!(instrument.text("group"), Some("   "));
    assert_eq!(instrument.text("secType"), Some(" a\n"));
}

#[test]
fn numbers_and_booleans() {
    let model = parse(&doc(concat!(
        "<ScanTypeList>",
        "<ScanType><respSizeLimit>#</respSizeLimit><snapshotSizeLimit>+12</snapshotSizeLimit>",
        "<supportsSorting>TRUE</supportsSorting><absoluteColumns>yes</absoluteColumns></ScanType>",
        "<ScanType><respSizeLimit> 5</respSizeLimit><snapshotSizeLimit>99999999999</snapshotSizeLimit>",
        "<supportsSorting></supportsSorting></ScanType>",
        "<ScanType respSizeLimit=\"7\"><respSizeLimit>12\n</respSizeLimit><snapshotSizeLimit>-3</snapshotSizeLimit></ScanType>",
        "</ScanTypeList>"
    )))
    .unwrap();
    let scan = item(&model, "scanTypeList", 0);
    assert_eq!(scan.get("respSizeLimit"), Some(&Value::Int(i32::MAX)));
    assert_eq!(scan.get("snapshotSizeLimit"), Some(&Value::Int(12)));
    assert_eq!(scan.get("supportsSorting"), Some(&Value::Bool(true)));
    assert_eq!(scan.get("absoluteColumns"), Some(&Value::Bool(false)));
    // Padded or too large is no number: the default stays.
    let scan = item(&model, "scanTypeList", 1);
    assert_eq!(scan.get("respSizeLimit"), Some(&Value::Int(i32::MAX)));
    assert_eq!(scan.get("snapshotSizeLimit"), Some(&Value::Int(i32::MAX)));
    assert_eq!(scan.get("supportsSorting"), Some(&Value::Bool(false)));
    // A bad text in the child element is not replaced by the attribute.
    let scan = item(&model, "scanTypeList", 2);
    assert_eq!(scan.get("respSizeLimit"), Some(&Value::Int(i32::MAX)));
    assert_eq!(scan.get("snapshotSizeLimit"), Some(&Value::Int(-3)));
}

#[test]
fn enumeration_is_read_from_the_attribute_first_and_falls_back_to_the_first_constant() {
    let model = parse(&doc(concat!(
        "<ColumnSets><ColumnSet><Columns>",
        "<Column displayType=\"BAR\"><displayType>LINE</displayType></Column>",
        "<Column><displayType>LINE</displayType></Column>",
        "<Column displayType=\"NOPE\"/>",
        "<ColumnSetRef colId=\"3\"/>",
        "<NotAColumn/>",
        "</Columns></ColumnSet></ColumnSets>"
    )))
    .unwrap();
    let columns = &item(&model, "columnSets", 0).child("columns").unwrap().items;
    let shown: Vec<(&str, &Value)> = columns
        .iter()
        .map(|v| match v {
            Value::Node(n) => (n.tag(), n.get("displayType").unwrap()),
            other => panic!("{other:?}"),
        })
        .collect();
    let constant = |name| Value::Enum { of: 0, name };
    // The tag names the class of an item of this list; an unknown tag
    // is skipped.
    assert_eq!(
        shown,
        vec![("Column", &constant("BAR")), ("Column", &constant("LINE")), ("Column", &constant("DATA")), ("ColumnSetRef", &constant("DATA"))]
    );
}

#[test]
fn list_of_one_class_takes_every_child_element() {
    let model = parse(&doc(
        "<InstrumentList><Other><name>a</name></Other><X type=\"scanner.ScanType\"><scanCode>S</scanCode></X><Y type=\"no.such\"><name>n</name></Y></InstrumentList>",
    ))
    .unwrap();
    let classes: Vec<&str> = (0..3).map(|i| item(&model, "instrumentList", i).class_name()).collect();
    // A `type` attribute that names a class wins; one that names none
    // is ignored.
    assert_eq!(classes, ["scanner.Instrument", "scanner.ScanType", "scanner.Instrument"]);
}

#[test]
fn list_of_several_classes_picks_by_tag_and_abstract_member_by_the_tag_of_its_element() {
    let xml = doc(concat!(
        "<FilterList><NotAFilter/>",
        "<SimpleFilter><id>A</id><AbstractField type=\"scanner.filter.IntField\" varName=\"field\"><code>c</code></AbstractField></SimpleFilter>",
        "<RangeFilter><id>B</id><DoubleField varName=\"min\"/><IntField varName=\"max\"/></RangeFilter>",
        "</FilterList>"
    ));
    let model = parse(&xml).unwrap();
    assert_eq!(items(&model, "filterList").len(), 2);
    let simple = item(&model, "filterList", 0);
    assert_eq!(simple.tag(), "SimpleFilter");
    assert_eq!(simple.child("field").unwrap().class_name(), "scanner.filter.IntField");
    // The field has a member named `varName`: the attribute fills it.
    assert_eq!(simple.child("field").unwrap().text("varName"), Some("field"));
    let range = item(&model, "filterList", 1);
    assert_eq!(range.child("min").unwrap().class_name(), "scanner.filter.DoubleField");
    assert_eq!(range.child("max").unwrap().class_name(), "scanner.filter.IntField");

    // Written under the declared class, with the actual one as `type`.
    let body = body_of(&xml);
    assert!(body.contains("\t\t\t<AbstractField type=\"scanner.filter.DoubleField\" varName=\"min\">\n\t\t\t\t<varName>min</varName>\n"), "{body}");
    assert!(body.contains("\t\t\t\t<maxValue>2147483647</maxValue>\n\t\t\t\t<minValue>-2147483648</minValue>\n\t\t\t</AbstractField>\n\t\t\t<skipValidation>false</skipValidation>\n"), "{body}");
}

#[test]
fn type_attribute_that_names_no_class_ends_the_class_level() {
    let scan = "<ScanType>\n\t\t\t<scanCode>S</scanCode>\n\t\t\t<absoluteColumns>false</absoluteColumns>\n";
    let rest = "\t\t\t<supportsSorting>true</supportsSorting>\n\t\t\t<respSizeLimit>2147483647</respSizeLimit>\n\t\t\t<snapshotSizeLimit>2147483647</snapshotSizeLimit>\n\t\t\t<searchDefault>false</searchDefault>\n\t\t</ScanType>\n";
    let wrap = |inner: &str| format!("<ScanParameterResponse>\n\t<ScanTypeList varName=\"scanTypeList\">\n\t\t{inner}\t</ScanTypeList>\n</ScanParameterResponse>\n");

    // The members after the bad one keep their defaults: the mobile
    // columns and the settings are lost.
    let body = body_of(&doc(
        "<ScanTypeList><ScanType><scanCode>S</scanCode><Columns type=\"no.such.Class\"/><MobileColumns/><settings>kept</settings></ScanType></ScanTypeList>",
    ));
    assert_eq!(body, wrap(&format!("{scan}{rest}")));

    // So does an array without class, or of a class that is not one
    // (ibx#599).
    for array in ["<Array varName=\"columns\"/>", "<Array type=\"no.such\" varName=\"columns\"/>"] {
        let body = body_of(&doc(&format!("<ScanTypeList><ScanType><scanCode>S</scanCode>{array}<MobileColumns/><settings>kept</settings></ScanType></ScanTypeList>")));
        assert_eq!(body, wrap(&format!("{scan}{rest}")), "{array}");
    }

    // A scalar type instead leaves that one member alone and goes on.
    let body = body_of(&doc(
        "<ScanTypeList><ScanType><scanCode>S</scanCode><Columns type=\"java.lang.String\">x</Columns><MobileColumns/></ScanType></ScanTypeList>",
    ));
    assert_eq!(body, wrap(&format!("{scan}\t\t\t<MobileColumns varName=\"mobileColumns\"/>\n{rest}")));

    // A list of another class than the declared one says so.
    let body = body_of(&doc(
        "<ScanTypeList><ScanType><scanCode>S</scanCode><Columns type=\"scanner.MobileColumns\"><MobileColumn><colId>4</colId></MobileColumn></Columns></ScanType></ScanTypeList>",
    ));
    assert_eq!(
        body,
        wrap(&format!(
            "{scan}\t\t\t<Columns varName=\"columns\" type=\"scanner.MobileColumns\">\n\t\t\t\t<MobileColumn>\n\t\t\t\t\t<colId>4</colId>\n\t\t\t\t</MobileColumn>\n\t\t\t</Columns>\n{rest}"
        ))
    );
}

/// A document with one filter that keeps its child elements as entries.
fn with_entries(entries: &str) -> String {
    doc(&format!(
        "<AdvancedScannerDefaults><InstrumentConfigurations><InstrumentConfiguration><instrumentType>STK</instrumentType>{entries}</InstrumentConfiguration></InstrumentConfigurations></AdvancedScannerDefaults>"
    ))
}

fn written_entries(inner: &str, end: &str) -> String {
    format!(
        "<ScanParameterResponse>\n\t<AdvancedScannerDefaults varName=\"advancedScannerDefaults\">\n\t\t<InstrumentConfigurations varName=\"instrumentConfigurations\">\n\t\t\t<InstrumentConfiguration>\n\t\t\t\t<instrumentType>STK</instrumentType>\n{inner}{end}\t\t\t</InstrumentConfiguration>\n\t\t</InstrumentConfigurations>\n\t</AdvancedScannerDefaults>\n</ScanParameterResponse>\n"
    )
}

#[test]
fn entries_keep_their_tags_order_and_build_limits() {
    let body = body_of(&with_entries("<AdvancedFilter><a minTwsBuild=\"950\" maxTwsBuild=\"x\">1 &lt; 2</a><a>dup</a><b maxTwsBuild=\"12\"/></AdvancedFilter>"));
    assert_eq!(
        body,
        written_entries(
            "\t\t\t\t<AdvancedFilter varName=\"filter\">\n\t\t\t\t\t<a minTwsBuild=\"950\">1 &lt; 2</a>\n\t\t\t\t\t<a>dup</a>\n\t\t\t\t\t<b maxTwsBuild=\"12\"></b>\n",
            "\t\t\t\t</AdvancedFilter>\n"
        )
    );
    // Without entry: start and end tag on two lines.
    let body = body_of(&with_entries("<AdvancedFilter/>"));
    assert_eq!(body, written_entries("\t\t\t\t<AdvancedFilter varName=\"filter\">\n", "\t\t\t\t</AdvancedFilter>\n"));
}

#[test]
fn entry_without_text_cuts_the_writing_of_its_holder() {
    // What was written stays, the end tag of the filter is missing and
    // the text is no longer well formed: so does the reference.
    let body = body_of(&with_entries("<AdvancedFilter><a>1</a><b><c/></b><d>2</d></AdvancedFilter>"));
    assert_eq!(body, written_entries("\t\t\t\t<AdvancedFilter varName=\"filter\">\n\t\t\t\t\t<a>1</a>\n", ""));
}

/// A filter whose field holds an object that is never written.
fn with_unwritten(inner: &str) -> String {
    doc(&format!(
        "<FilterList varName=\"filterList\"><TripleComboFilter><TripleComboField><TripleValues varName=\"tripleMap\">{inner}</TripleValues></TripleComboField></TripleComboFilter></FilterList>"
    ))
}

#[test]
fn object_that_is_never_written_still_closes_the_start_tag() {
    // The text of the reference 10.51 (and 10.40) for this document.
    let body = body_of(&with_unwritten("<x name=\"n\">t<y name=\"m\"><z name=\"\"/></y></x><!-- c -->"));
    assert_eq!(
        body,
        concat!(
            "<ScanParameterResponse>\n\t<FilterList varName=\"filterList\">\n\t\t<TripleComboFilter>\n",
            "\t\t\t<category>General</category>\n\t\t\t<TripleComboField varName=\"triField\">\n",
            "\t\t\t\t<dontAllowClearAll>false</dontAllowClearAll>\n\t\t\t</TripleComboField>\n",
            "\t\t</TripleComboFilter>\n\t</FilterList>\n</ScanParameterResponse>\n",
        )
    );
}

#[test]
fn object_that_is_never_written_stops_the_reading_without_a_name_on_its_first_three_levels() {
    for inner in ["<x/>", "<x name=\"n\"><y/></x>", "<x name=\"n\"><y name=\"m\"><z/></y></x>"] {
        assert!(rebuild(&with_unwritten(inner)).is_none(), "{inner}");
    }
    // Below the third level, nothing is asked for.
    assert!(rebuild(&with_unwritten("<x name=\"n\"><y name=\"m\"><z name=\"o\"><w/></z></y></x>")).is_some());
}

#[test]
fn text_list_and_escaping() {
    let body = body_of(&doc(concat!(
        "<SidecarScannerDefaults><SidecarScannerTemplateList><SidecarScannerTemplate>",
        "<name>&amp;&lt;&gt;&apos;&quot;&#10;&#9;&#13;\u{e9}</name>",
        "<ArString varName=\"regions\"><String>US</String><x>EU</x><y><z/></y></ArString>",
        "</SidecarScannerTemplate></SidecarScannerTemplateList></SidecarScannerDefaults>"
    )));
    // Line feed as a reference; tab, carriage return and the rest raw.
    assert!(body.contains("\t\t\t\t<name>&amp;&lt;&gt;&apos;&quot;&#10;\t\r\u{e9}</name>\n"), "{body}");
    // Every child element with a text is an item, under one tag.
    assert!(body.contains("\t\t\t\t<ArString varName=\"regions\">\n\t\t\t\t\t<String>US</String>\n\t\t\t\t\t<String>EU</String>\n\t\t\t\t</ArString>\n"), "{body}");
}

#[test]
fn names_are_compared_as_written() {
    // A prefixed element or attribute is not the plain one.
    let body = body_of(concat!(
        "<r xmlns:p=\"urn:x\"><p:InstrumentList><Instrument><name>lost</name></Instrument></p:InstrumentList>",
        "<InstrumentList varName=\"fullInstrumentList\"><Instrument p:name=\"attr\"><p:name>N</p:name></Instrument></InstrumentList></r>"
    ));
    assert_eq!(
        body,
        "<ScanParameterResponse>\n\t<InstrumentList varName=\"fullInstrumentList\">\n\t\t<Instrument>\n\t\t\t<cloudScanNotSupported>false</cloudScanNotSupported>\n\t\t</Instrument>\n\t</InstrumentList>\n</ScanParameterResponse>\n"
    );
}

#[test]
fn character_data_as_the_reference_parser_gives_it() {
    let model = parse(&doc(concat!(
        "<InstrumentList><Instrument name=\"a\r\nb\tc\">",
        "<group>x\r\ny\rz</group><secType><![CDATA[a<b]]> t</secType><filters><?pi x?>after</filters>",
        "<shortName><!--only--></shortName><context><![CDATA[]]></context>",
        "</Instrument></InstrumentList>"
    )))
    .unwrap();
    let instrument = item(&model, "instrumentList", 0);
    // Line ends become line feeds in text, blanks in an attribute.
    assert_eq!(instrument.text("group"), Some("x\ny\nz"));
    assert_eq!(instrument.text("name"), Some("a b c"));
    assert_eq!(instrument.text("secType"), Some("a<b t"));
    // A processing instruction counts as a child but has no text.
    assert_eq!(instrument.text("filters"), Some("after"));
    assert_eq!(instrument.text("shortName"), Some("only"));
    assert_eq!(instrument.text("context"), Some(""));
}

#[test]
fn refused_documents() {
    for xml in [
        // Not well formed.
        "<a><b></a>".to_string(),
        String::new(),
        "<r><p:InstrumentList/></r>".to_string(),
        doc("<InstrumentList><Instrument><name>&#1;</name></Instrument></InstrumentList>"),
        // An array of a class where an object is expected.
        doc("<ScanTypeList><ScanType><Array type=\"java.lang.String\" varName=\"columns\"/></ScanType></ScanTypeList>"),
        doc("<ScanTypeList><ScanType><Array type=\"scanner.Columns\" varName=\"columns\"/></ScanType></ScanTypeList>"),
        // An abstract member whose element names no class.
        doc("<FilterList><SimpleFilter><NoSuchField varName=\"field\"/></SimpleFilter></FilterList>"),
        // A default the model does not describe is left in place.
        doc("<LocationTree><Location><locationCode>L</locationCode><LocationTree type=\"no.such\"/></Location></LocationTree>"),
    ] {
        assert!(rebuild(&xml).is_none(), "{xml}");
    }
}

// ── The model between reading and writing ──

#[test]
fn model_can_be_edited_before_it_is_written() {
    let mut model = parse(&doc(concat!(
        "<InstrumentList varName=\"instrumentList\"><Instrument><name>A</name></Instrument></InstrumentList>",
        "<ScanTypeList><ScanType><scanCode>KEEP</scanCode></ScanType><ScanType><scanCode>DROP</scanCode></ScanType></ScanTypeList>"
    )))
    .unwrap();

    let scans = model.root.child_mut("scanTypeList").unwrap();
    scans.items.retain(|v| matches!(v, Value::Node(n) if n.text("scanCode") != Some("DROP")));
    let Value::Node(kept) = &mut scans.items[0] else { panic!() };
    assert!(kept.set("displayName", Value::Text("Shown".into())));
    assert!(!kept.set("noSuchMember", Value::Null));

    let mut added = Node::new("scanner.Instrument").unwrap();
    added.set("name", Value::Text("B".into()));
    *added.get_mut("type").unwrap() = Value::Text("STK".into());
    model.root.child_mut("instrumentList").unwrap().items.push(Value::Node(Box::new(added)));

    // The same as reading the document that already has the changes.
    let edited = doc(concat!(
        "<InstrumentList varName=\"instrumentList\"><Instrument><name>A</name></Instrument>",
        "<Instrument><name>B</name><type>STK</type></Instrument></InstrumentList>",
        "<ScanTypeList><ScanType><scanCode>KEEP</scanCode><displayName>Shown</displayName></ScanType></ScanTypeList>"
    ));
    assert_eq!(model, parse(&edited).unwrap());
    assert_eq!(write(&model), rebuild(&edited));
    assert!(Node::new("scanner.filter.AbstractField").is_none());
    assert!(Node::new("no.such.Class").is_none());
}

#[test]
fn virtual_filter_has_its_display_name_once() {
    // The reference 10.40 declared it at two class levels and wrote it
    // twice; the reference 10.51 only at the level of every filter
    // (ibx#716).
    let xml = doc("<FilterList varName=\"filterList\"><VirtualFilter><displayName>D</displayName></VirtualFilter></FilterList>");
    let model = parse(&xml).unwrap();
    let filter = item(&model, "filterList", 0);
    let slots: Vec<usize> = filter.slots("displayName").collect();
    assert_eq!(slots.len(), 1);
    assert_eq!(filter.members[slots[0]], Value::Text("D".into()));
    assert_eq!(body_of(&xml).matches("<displayName>D</displayName>").count(), 1);
}
