//! Reading the scanner parameters into the model, by the rules of the
//! reference (ibx#571).
//!
//! The reader is driven by the classes of the model, not by the
//! document: for each member of a class it looks for the element or the
//! attribute that feeds it. What the model does not ask for is ignored.

use std::borrow::Cow;

use roxmltree::NodeType;

use super::tables::{CLASSES, ENUMS, ROOT};
use super::{class_in_package, class_named, enum_in_package, enum_named, simple_name};
use super::{ClassId, Entry, Items, Kind, Model, Node, Refused, Ty, Value};

/// Why a document was not read whole.
#[derive(Debug)]
enum Failure {
    /// The reference fails on it, or the model does not cover it.
    Refused,
    /// The reference's reader throws to its caller, which keeps what was
    /// read so far (ibx#599).
    Thrown,
}

type El<'a, 'i> = roxmltree::Node<'a, 'i>;

/// Names of scalar types. As the `type` attribute of an object member
/// they leave the member as it is.
const SCALAR_TYPES: &[&str] = &[
    "int",
    "java.lang.Integer",
    "byte",
    "java.lang.Byte",
    "boolean",
    "java.lang.Boolean",
    "long",
    "java.lang.Long",
    "char",
    "java.lang.Character",
    "double",
    "java.lang.Double",
    "float",
    "java.lang.Float",
    "java.lang.String",
    "java.awt.Color",
    "java.lang.Class",
];

/// Read a document. The name of its root element is not checked.
///
/// Where the reference's reader throws to its caller, the caller goes on
/// with the root as far as it was read: the members read before the one
/// that failed (ibx#599). The second value is false then.
pub(super) fn read(text: &str) -> Result<(Model, bool), Refused> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, options).map_err(|_| Refused)?;
    let reader = Reader { src: text };
    let mut root = Node::of(ROOT).ok_or(Refused)?;
    let whole = match reader.object(&mut root, doc.root_element()) {
        Ok(()) => true,
        Err(Failure::Thrown) => false,
        Err(Failure::Refused) => return Err(Refused),
    };
    Ok((Model { root }, whole))
}

struct Reader<'i> {
    /// The document text: element names are taken from it, prefix
    /// included, as the reference compares the name as written.
    src: &'i str,
}

impl Reader<'_> {
    /// Name of an element as written in the document.
    fn name<'a>(&'a self, el: El<'a, '_>) -> &'a str {
        let start = el.range().start;
        match self.src.get(start + 1..) {
            Some(rest) if self.src.as_bytes()[start] == b'<' => {
                let end = rest.find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/').unwrap_or(rest.len());
                &rest[..end]
            }
            _ => el.tag_name().name(),
        }
    }

    /// First child element with this name; with `bare`, one that has no
    /// `varName` attribute (or an empty one).
    fn child_named<'a, 'i>(&self, el: El<'a, 'i>, name: &str, bare: bool) -> Option<El<'a, 'i>> {
        el.children()
            .filter(|c| c.is_element())
            .find(|c| self.name(*c) == name && !(bare && attr(*c, "varName").is_some_and(|v| !v.is_empty())))
    }

    /// Text of the first child element with this name.
    fn child_text<'a>(&self, el: El<'a, '_>, name: &str) -> Option<Cow<'a, str>> {
        self.text(self.child_named(el, name, false)?)
    }

    /// Text of an element. No child: empty. One child: its text, or
    /// nothing when it is an element. Several: all character data put
    /// together, comments included; when that is empty, what the first
    /// child gives. An element named `null` gives nothing.
    ///
    /// Text and CDATA that touch count as one child: nothing can tell
    /// them apart in the result.
    fn text<'a>(&self, el: El<'a, '_>) -> Option<Cow<'a, str>> {
        if self.name(el) == "null" {
            return None;
        }
        if let Some(only) = el.first_child().filter(|c| Some(*c) == el.last_child()) {
            return match only.node_type() {
                NodeType::Text | NodeType::Comment => Some(Cow::Borrowed(only.text().unwrap_or(""))),
                _ => None,
            };
        }
        let mut count = 0;
        let mut all = String::new();
        // What the first child gives; a run of text nodes is one child.
        let mut first: Option<String> = None;
        let mut in_text = false;
        for kid in el.children() {
            let data = match kid.node_type() {
                NodeType::Text | NodeType::Comment => kid.text().unwrap_or(""),
                _ => {
                    count += 1;
                    in_text = false;
                    continue;
                }
            };
            let is_text = kid.node_type() == NodeType::Text;
            if is_text && data.is_empty() {
                continue;
            }
            if !(is_text && in_text) {
                count += 1;
                if count == 1 {
                    first = Some(String::new());
                }
            }
            if count == 1 {
                if let Some(first) = first.as_mut() {
                    first.push_str(data);
                }
            }
            in_text = is_text;
            all.push_str(data);
        }
        if count > 1 && !all.is_empty() {
            Some(Cow::Owned(all))
        } else if count == 0 {
            Some(Cow::Borrowed(""))
        } else {
            first.map(Cow::Owned)
        }
    }

    /// Fill an object from its element, by the kind of its class.
    fn object(&self, node: &mut Node, el: El) -> Result<(), Failure> {
        let class = &CLASSES[node.class() as usize];
        match &class.kind {
            Kind::List(items) => self.list(node, el, items),
            Kind::Flex(classes) => self.flexible_list(node, el, classes),
            Kind::Entries => {
                node.entries = el
                    .children()
                    .filter(|c| c.is_element())
                    .map(|c| Entry {
                        name: self.name(c).to_string(),
                        text: self.text(c).map(Cow::into_owned),
                        min_build: attr(c, "minTwsBuild").and_then(int_text),
                        max_build: attr(c, "maxTwsBuild").and_then(int_text),
                    })
                    .collect();
                Ok(())
            }
            // Never written, so what it holds is not read either.
            Kind::Silent => Ok(()),
            Kind::Plain => {
                // Superclass members first. A level that stops early
                // does not stop the others.
                let mut base = 0;
                for &level in class.chain {
                    self.members(level, base, node, el)?;
                    base += CLASSES[level as usize].members.len();
                }
                Ok(())
            }
        }
    }

    /// Read the members one class level declares; `base` is the position
    /// of its first member in the object. Returns false when the level
    /// was left early: its remaining members keep their defaults.
    fn members(&self, level: ClassId, base: usize, node: &mut Node, el: El) -> Result<bool, Failure> {
        for (i, member) in CLASSES[level as usize].members.iter().enumerate() {
            let slot = base + i;
            let (decl, decl_tag) = match member.ty {
                Ty::Str | Ty::Int | Ty::Bool => {
                    // Child element first, attribute second; absent in
                    // both, the default stays.
                    let text = self.child_text(el, member.tag).or_else(|| attr(el, attribute_of(member.tag)).map(Cow::Borrowed));
                    let Some(text) = text else { continue };
                    if matches!(member.ty, Ty::Str) && is_blank_with_break(&text) {
                        node.members[slot] = Value::Text(String::new());
                    } else if let Some(v) = scalar(member.ty, &text) {
                        node.members[slot] = v;
                    }
                    continue;
                }
                Ty::Enum(of) => {
                    // The other way round: attribute first. Absent or
                    // unknown gives the first constant.
                    let text = attr(el, attribute_of(member.tag)).map(Cow::Borrowed).or_else(|| self.child_text(el, member.tag));
                    node.members[slot] = constant(of, text.as_deref());
                    continue;
                }
                Ty::Obj(c) => (Some(c), simple_name(CLASSES[c as usize].name)),
                Ty::Foreign(tag) => (None, tag),
            };
            // An object member: the element whose `varName` is the
            // member's name, else the first element named like the
            // declared class that has no `varName`. One element can
            // therefore feed several members.
            let found = el
                .children()
                .filter(|c| c.is_element())
                .find(|c| attr(*c, "varName").is_some_and(|v| v == member.tag))
                .or_else(|| self.child_named(el, decl_tag, true));
            let Some(sub) = found else {
                // Not found: no value, whatever the default was.
                node.members[slot] = Value::Null;
                continue;
            };
            let sub_tag = self.name(sub);
            if sub_tag == "Array" {
                // An array of the class its `type` names. None, or a name
                // that is no class: the level ends here. A class: the
                // member cannot hold the array and the reader throws.
                let element = attr(sub, "type").unwrap_or("");
                if element.is_empty() || !(SCALAR_TYPES.contains(&element) || enum_named(element).is_some() || class_named(element).is_some()) {
                    return Ok(false);
                }
                return Err(Failure::Thrown);
            }
            let mut class = decl;
            let type_attr = attr(sub, "type").unwrap_or("");
            if !type_attr.is_empty() {
                if SCALAR_TYPES.contains(&type_attr) || enum_named(type_attr).is_some() {
                    continue;
                }
                match class_named(type_attr) {
                    Some(c) => class = Some(c),
                    // Names no class: the level ends here.
                    None => return Ok(false),
                }
            }
            let mut class = class.ok_or(Failure::Refused)?;
            let name = CLASSES[class as usize].name;
            if CLASSES[class as usize].abs {
                // An abstract class stands for the class named like the
                // element, in its own package.
                let package = name.rfind('.').map_or("", |i| &name[..i]);
                // None of that name: the reader throws.
                class = class_in_package(package, sub_tag).ok_or(Failure::Thrown)?;
            }
            let mut value = Node::of(class).ok_or(Failure::Refused)?;
            self.object(&mut value, sub)?;
            node.members[slot] = Value::Node(Box::new(value));
        }
        Ok(true)
    }

    /// A list: every child element is an item, in document order. Then
    /// the scalar members the list class declares itself are read.
    fn list(&self, node: &mut Node, el: El, items: &Items) -> Result<(), Failure> {
        node.items.clear();
        for c in el.children().filter(|c| c.is_element()) {
            let class = match *items {
                Items::Text => {
                    if let Some(text) = self.text(c) {
                        node.items.push(Value::Text(text.into_owned()));
                    }
                    continue;
                }
                Items::Class(class) => class,
                Items::Package(package) => {
                    // The `type` attribute, else the tag, names the
                    // class in the package; an unknown name is skipped.
                    let name = attr(c, "type").filter(|t| !t.is_empty()).unwrap_or_else(|| self.name(c));
                    if let Some(class) = class_in_package(package, name) {
                        class
                    } else {
                        if let Some(of) = enum_in_package(package, name) {
                            node.items.push(constant(of, self.text(c).as_deref()));
                        }
                        continue;
                    }
                }
            };
            node.items.push(Value::Node(Box::new(self.item(c, class)?)));
        }
        let class = &CLASSES[node.class() as usize];
        let base = node.members.len() - class.members.len();
        for (i, member) in class.members.iter().enumerate() {
            match member.ty {
                Ty::Str | Ty::Int | Ty::Bool => {
                    let text = self.child_text(el, member.tag).or_else(|| attr(el, attribute_of(member.tag)).map(Cow::Borrowed));
                    if let Some(v) = text.and_then(|t| scalar(member.ty, &t)) {
                        node.members[base + i] = v;
                    }
                }
                Ty::Enum(of) => node.members[base + i] = constant(of, self.child_text(el, member.tag).as_deref()),
                Ty::Obj(_) | Ty::Foreign(_) => {}
            }
        }
        Ok(())
    }

    /// A list whose items are of several classes: the `type` attribute
    /// when it names a class, else the class of the list with the tag's
    /// name; no match, the element is skipped.
    fn flexible_list(&self, node: &mut Node, el: El, classes: &[ClassId]) -> Result<(), Failure> {
        node.items.clear();
        for c in el.children().filter(|c| c.is_element()) {
            let tag = self.name(c);
            let class = match attr(c, "type").and_then(class_named) {
                Some(class) => class,
                None => match classes.iter().find(|&&k| simple_name(CLASSES[k as usize].name) == tag) {
                    Some(&class) => class,
                    // A `type` that names an enumeration is refused
                    // by `item`, as for any list.
                    None if attr(c, "type").and_then(enum_named).is_none() => continue,
                    None => return Err(Failure::Refused),
                },
            };
            node.items.push(Value::Node(Box::new(self.item(c, class)?)));
        }
        Ok(())
    }

    /// A new item from its element: a `type` attribute that names a
    /// class wins over the class of the list; one that names none is
    /// ignored.
    fn item(&self, el: El, mut class: ClassId) -> Result<Node, Failure> {
        if let Some(t) = attr(el, "type").filter(|t| !t.is_empty()) {
            if let Some(named) = class_named(t) {
                class = named;
            } else if enum_named(t).is_some() {
                return Err(Failure::Refused);
            }
        }
        let mut item = Node::of(class).ok_or(Failure::Refused)?;
        self.object(&mut item, el)?;
        Ok(item)
    }
}

/// Attribute name of a member: its element name, except that `type`
/// is taken by the class name of an element.
fn attribute_of(tag: &str) -> &str {
    if tag == "type" {
        "m_type"
    } else {
        tag
    }
}

/// Only characters up to the space, with a line break or a tab among
/// them: such a text is read as the empty text.
fn is_blank_with_break(s: &str) -> bool {
    s.chars().all(|c| c <= ' ') && s.contains(['\n', '\r', '\t'])
}

/// Whole number: optional sign and digits, nothing else.
fn int_text(s: &str) -> Option<i32> {
    s.parse().ok()
}

/// The value of a scalar member from its text; `None` leaves the member
/// as it is. Whole number: `#` is the largest one; blank, padded or too
/// large is not a number. Boolean: `true` in any case, all else false.
fn scalar(ty: Ty, text: &str) -> Option<Value> {
    match ty {
        Ty::Str => Some(Value::Text(text.to_string())),
        Ty::Bool => Some(Value::Bool(text.eq_ignore_ascii_case("true"))),
        Ty::Int if text == "#" => Some(Value::Int(i32::MAX)),
        Ty::Int => int_text(text).map(Value::Int),
        _ => None,
    }
}

/// The constant with this name, else the first one.
fn constant(of: u16, text: Option<&str>) -> Value {
    let consts = ENUMS[of as usize].consts;
    let name = text.and_then(|t| consts.iter().find(|c| **c == t)).unwrap_or(&consts[0]);
    Value::Enum { of, name }
}

/// An attribute by its name as written; one with a prefix is never
/// asked for.
fn attr<'a>(el: El<'a, '_>, name: &str) -> Option<&'a str> {
    el.attributes().find(|a| a.namespace().is_none() && a.name() == name).map(|a| a.value())
}
