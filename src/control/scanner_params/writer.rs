//! Writing the model of the scanner parameters, by the rules of the
//! reference (ibx#571).
//!
//! One line per element, one tab per depth, line feeds only. An object
//! writes the members of its class levels, superclass first, in declared
//! order; a member without value is left out, a scalar is always
//! written. The only attributes are `varName` (the member an element
//! belongs to) and `type` (the class, when the tag does not tell it).

use super::tables::{CLASSES, ENUMS};
use super::{simple_name, ClassId, Kind, Model, Node, Ty, Value};

/// Why the writing of a value ended early.
pub(super) enum Stop {
    /// An entry without text: the object holding the entries gives up
    /// where it is, and the class level being written one step up ends
    /// there too.
    Abort,
    /// The model cannot write this value.
    Refused,
}

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

pub(super) fn write(model: &Model) -> Result<String, Stop> {
    let mut out = String::with_capacity(1 << 16);
    out.push_str(HEADER);
    let root = &model.root;
    object(&mut out, root, 0, root.tag(), None)?;
    Ok(out)
}

fn indent(out: &mut String, depth: usize) {
    out.extend(std::iter::repeat_n('\t', depth));
}

/// Text of an element: the five markup characters and the line feed are
/// written as references; tab, carriage return and everything else go
/// out as they are.
fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            _ => out.push(c),
        }
    }
}

fn line(out: &mut String, depth: usize, tag: &str, text: impl FnOnce(&mut String)) {
    indent(out, depth);
    out.push('<');
    out.push_str(tag);
    out.push('>');
    text(out);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// What a member is declared as, for the tag and the `type` attribute
/// of the element that holds its value.
#[derive(Clone, Copy)]
struct Decl {
    tag: &'static str,
    class: Option<ClassId>,
}

/// Write one value. `open` tells that the start tag of the enclosing
/// element is not closed yet: the first value written closes it.
/// Returns whether something was written.
fn value(out: &mut String, v: &Value, depth: usize, tag: &str, decl: Decl, var_name: Option<&str>, open: bool) -> Result<bool, Stop> {
    let close = |out: &mut String| {
        if open {
            out.push_str(">\n");
        }
    };
    match v {
        Value::Null => return Ok(false),
        Value::Opaque => return Err(Stop::Refused),
        Value::Text(s) => {
            close(out);
            line(out, depth, tag, |out| escape(out, s));
        }
        Value::Int(i) => {
            close(out);
            line(out, depth, tag, |out| out.push_str(&i.to_string()));
        }
        Value::Bool(b) => {
            close(out);
            line(out, depth, tag, |out| out.push_str(if *b { "true" } else { "false" }));
        }
        Value::Enum { name, .. } => {
            close(out);
            line(out, depth, tag, |out| out.push_str(name));
        }
        Value::Node(node) => {
            close(out);
            match CLASSES[node.class() as usize].kind {
                Kind::Entries => entries(out, node, depth, var_name)?,
                Kind::Silent => {}
                Kind::List(_) | Kind::Flex(_) => list(out, node, depth, decl, var_name)?,
                Kind::Plain => object(out, node, depth, decl.tag, var_name)?,
            }
        }
    }
    Ok(true)
}

/// An object: the tag is the declared class of the member (its own
/// class for an item and for the root), with a `type` attribute when
/// the object is of another class. The first member written closes the
/// start tag; with none the element is empty.
fn object(out: &mut String, node: &Node, depth: usize, tag: &str, var_name: Option<&str>) -> Result<(), Stop> {
    let class = &CLASSES[node.class() as usize];
    indent(out, depth);
    out.push('<');
    out.push_str(tag);
    if simple_name(class.name) != tag {
        attribute(out, "type", class.name);
    }
    if let Some(name) = var_name {
        attribute(out, "varName", name);
    }
    let mut any = false;
    let mut base = 0;
    for &level in class.chain {
        let members = CLASSES[level as usize].members;
        match level_members(out, node, depth, base, members, &mut any) {
            // What the level wrote stays, the rest of it is not written.
            Ok(()) | Err(Stop::Abort) => {}
            Err(stop) => return Err(stop),
        }
        base += members.len();
    }
    if any {
        indent(out, depth);
        out.push_str("</");
        out.push_str(tag);
        out.push_str(">\n");
    } else {
        out.push_str("/>\n");
    }
    Ok(())
}

fn level_members(out: &mut String, node: &Node, depth: usize, base: usize, members: &'static [super::Member], any: &mut bool) -> Result<(), Stop> {
    for (i, member) in members.iter().enumerate() {
        let decl = match member.ty {
            Ty::Obj(c) => Decl { tag: simple_name(CLASSES[c as usize].name), class: Some(c) },
            Ty::Foreign(tag) => Decl { tag, class: None },
            Ty::Enum(of) => Decl { tag: simple_name(ENUMS[of as usize].name), class: None },
            Ty::Str | Ty::Int | Ty::Bool => Decl { tag: member.tag, class: None },
        };
        if value(out, &node.members[base + i], depth + 1, member.tag, decl, Some(member.tag), !*any)? {
            *any = true;
        }
    }
    Ok(())
}

/// A list: tag and `varName` as for an object, a `type` attribute when
/// the list is not of the declared class, then the items, each under
/// the name of its own class. The members of the list class itself are
/// not written.
fn list(out: &mut String, node: &Node, depth: usize, decl: Decl, var_name: Option<&str>) -> Result<(), Stop> {
    indent(out, depth);
    out.push('<');
    out.push_str(decl.tag);
    if let Some(name) = var_name {
        attribute(out, "varName", name);
    }
    if decl.class != Some(node.class()) {
        attribute(out, "type", node.class_name());
    }
    if node.items.is_empty() {
        out.push_str("/>\n");
        return Ok(());
    }
    out.push_str(">\n");
    for item in &node.items {
        let (tag, class) = match item {
            Value::Node(n) => (n.tag(), Some(n.class())),
            Value::Enum { of, .. } => (simple_name(ENUMS[*of as usize].name), None),
            Value::Text(_) => ("String", None),
            Value::Int(_) => ("Integer", None),
            Value::Bool(_) => ("Boolean", None),
            Value::Null | Value::Opaque => ("", None),
        };
        value(out, item, depth + 1, tag, Decl { tag, class }, None, false)?;
    }
    indent(out, depth);
    out.push_str("</");
    out.push_str(decl.tag);
    out.push_str(">\n");
    Ok(())
}

/// An object that keeps its child elements as they came: one line per
/// entry with its build limits as attributes. The end tag is always
/// written, so an object without entry takes two lines.
fn entries(out: &mut String, node: &Node, depth: usize, var_name: Option<&str>) -> Result<(), Stop> {
    let tag = node.tag();
    indent(out, depth);
    out.push('<');
    out.push_str(tag);
    if let Some(name) = var_name {
        attribute(out, "varName", name);
    }
    out.push_str(">\n");
    for entry in &node.entries {
        let text = entry.text.as_deref().ok_or(Stop::Abort)?;
        indent(out, depth + 1);
        out.push('<');
        out.push_str(&entry.name);
        if let Some(n) = entry.min_build {
            attribute(out, "minTwsBuild", &n.to_string());
        }
        if let Some(n) = entry.max_build {
            attribute(out, "maxTwsBuild", &n.to_string());
        }
        out.push('>');
        escape(out, text);
        out.push_str("</");
        out.push_str(&entry.name);
        out.push_str(">\n");
    }
    indent(out, depth);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
    Ok(())
}

fn attribute(out: &mut String, name: &str, text: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(text);
    out.push('"');
}
