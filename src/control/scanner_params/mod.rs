//! The scanner parameters document as the reference hands it to its
//! clients (ibx#571).
//!
//! The reference does not pass the server's document on: it reads it
//! into an object model and writes the model out again. The text that
//! comes out is larger and ordered differently: the sections follow the
//! declared members of the model, every attribute becomes a child
//! element, members the server left out appear with their defaults, and
//! one element can feed several members.
//!
//! [`parse`] reads the server text into a [`Model`], [`write`] writes a
//! model, [`rebuild`] does both. Between the two the reference removes,
//! replaces and adds items, in part by what the session has:
//! [`for_client`] gives the text a client of that [`Session`] receives.
//!
//! `tables.rs` holds the classes of the model and `session_tables.rs`
//! what the steps between reading and writing compare against; both are
//! generated. The rules of reading are in `reader.rs`, those of writing
//! in `writer.rs`, the steps between the two in `session.rs`.

mod reader;
mod session;
mod session_tables;
#[cfg(test)]
mod session_tests;
mod tables;
#[cfg(test)]
mod tests;
mod writer;

pub use session::Session;

use tables::{CLASSES, ENUMS};

/// Index of a class in the generated table.
pub(crate) type ClassId = u16;

/// One class of the model (generated).
struct Class {
    /// Full name, as a `type` attribute gives it; the tag of an element
    /// is the part after the last dot.
    name: &'static str,
    /// The class levels whose members are read and written, top-most
    /// superclass first, the class itself last. Empty for a class that
    /// has its own form.
    chain: &'static [ClassId],
    kind: Kind,
    /// Whether the reader can make an object of the class.
    make: bool,
    /// An abstract class as a member type: the tag of the element names
    /// the class to make, in the same package.
    abs: bool,
    /// Member values of a fresh object that differ from the ones the
    /// members declare: position among all members of `chain`, value.
    over: &'static [(u16, Def)],
    /// The members the class declares itself, in declared order.
    members: &'static [Member],
}

/// How a class is read and written.
enum Kind {
    /// By its members.
    Plain,
    /// A list with one rule for the class of its items.
    List(Items),
    /// A list whose items are of one of these classes, chosen by tag.
    Flex(&'static [ClassId]),
    /// One entry per child element, whatever its tag.
    Entries,
    /// Never written.
    Silent,
}

/// The class of the items of a list.
enum Items {
    /// Every child element is an item of this class.
    Class(ClassId),
    /// Every child element is a text item.
    Text,
    /// The tag of a child element names its class in this package.
    Package(&'static str),
}

/// One member of a class (generated).
struct Member {
    /// Element name of the member.
    tag: &'static str,
    ty: Ty,
    /// Value in a fresh object.
    def: Def,
}

/// Declared type of a member.
#[derive(Clone, Copy)]
enum Ty {
    Str,
    Int,
    Bool,
    /// Index in the table of enumerations.
    Enum(u16),
    Obj(ClassId),
    /// A class outside the model, known by its tag only.
    Foreign(&'static str),
}

/// A member by the class level that declares it and its position among
/// the members of that level (generated).
#[derive(Clone, Copy)]
struct Slot {
    level: ClassId,
    at: u16,
}

/// Value of a member in a fresh object.
#[derive(Clone, Copy)]
enum Def {
    Null,
    Str(&'static str),
    Int(i32),
    Bool(bool),
    Enum(&'static str),
    /// An object the model does not describe.
    Opaque,
}

/// An enumeration of the model (generated).
struct Enum {
    name: &'static str,
    /// The constants; the first one stands for any unknown text.
    consts: &'static [&'static str],
}

/// The part of a class name after the last dot.
fn simple_name(name: &str) -> &str {
    name.rfind('.').map_or(name, |i| &name[i + 1..])
}

/// The class with this full name.
fn class_named(name: &str) -> Option<ClassId> {
    CLASSES.iter().position(|c| c.name == name).map(|i| i as ClassId)
}

/// The class `tag` of package `package`.
fn class_in_package(package: &str, tag: &str) -> Option<ClassId> {
    CLASSES.iter().position(|c| is_in_package(c.name, package, tag)).map(|i| i as ClassId)
}

/// The enumeration with this full name.
fn enum_named(name: &str) -> Option<u16> {
    ENUMS.iter().position(|e| e.name == name).map(|i| i as u16)
}

/// The enumeration `tag` of package `package`.
fn enum_in_package(package: &str, tag: &str) -> Option<u16> {
    ENUMS.iter().position(|e| is_in_package(e.name, package, tag)).map(|i| i as u16)
}

fn is_in_package(name: &str, package: &str, tag: &str) -> bool {
    name.len() == package.len() + 1 + tag.len()
        && name.starts_with(package)
        && name.ends_with(tag)
        && name.as_bytes()[package.len()] == b'.'
}

/// A value of the model: a member of an object or an item of a list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// No value: the member is not written.
    Null,
    Text(String),
    Int(i32),
    Bool(bool),
    /// A constant of the enumeration `of`.
    Enum { of: u16, name: &'static str },
    Node(Box<Node>),
    /// A default the model does not describe; a document that leaves
    /// one in place cannot be written.
    Opaque,
}

/// One entry of an object that keeps its child elements as they come.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    /// Tag of the element.
    pub(crate) name: String,
    /// Its text; `None` when the element holds another element instead.
    pub(crate) text: Option<String>,
    /// The two build limits the element may carry as attributes.
    pub(crate) min_build: Option<i32>,
    pub(crate) max_build: Option<i32>,
}

/// One object of the model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    class: ClassId,
    /// The members of all class levels, top-most superclass first, in
    /// declared order.
    pub(crate) members: Vec<Value>,
    /// The items, when the class is a list.
    pub(crate) items: Vec<Value>,
    /// The entries, when the class keeps its child elements as they come.
    pub(crate) entries: Vec<Entry>,
}

impl Node {
    /// A fresh object of a class, its members at their defaults; `None`
    /// when the reader cannot make one.
    fn of(class: ClassId) -> Option<Node> {
        CLASSES[class as usize].make.then(|| Node::fresh(class))
    }

    /// A fresh object of a class, also of one that only the steps
    /// between reading and writing make.
    fn fresh(class: ClassId) -> Node {
        let c = &CLASSES[class as usize];
        let mut members = Vec::with_capacity(c.chain.iter().map(|&l| CLASSES[l as usize].members.len()).sum());
        for &level in c.chain {
            members.extend(CLASSES[level as usize].members.iter().map(|m| fresh(m.def, m.ty)));
        }
        for &(slot, def) in c.over {
            let ty = member_at(c, slot as usize).map_or(Ty::Str, |m| m.ty);
            members[slot as usize] = fresh(def, ty);
        }
        Node { class, members, items: Vec::new(), entries: Vec::new() }
    }

    fn class(&self) -> ClassId {
        self.class
    }

    /// Full name of the class.
    pub(crate) fn class_name(&self) -> &'static str {
        CLASSES[self.class as usize].name
    }

    /// Tag of the object as an item of a list.
    pub(crate) fn tag(&self) -> &'static str {
        simple_name(self.class_name())
    }
}

/// Members by their element name, for the tests: the steps between
/// reading and writing name a member by the class level that declares it.
#[cfg(test)]
impl Node {
    /// A fresh object of the class with this full name; `None` when the
    /// reader cannot make one.
    pub(crate) fn new(class_name: &str) -> Option<Node> {
        Node::of(class_named(class_name)?)
    }

    /// Positions in `members` of the members with this element name.
    /// A subclass can declare a name its superclass has too: both are
    /// read from the same element and both are written.
    pub(crate) fn slots<'a>(&self, tag: &'a str) -> impl Iterator<Item = usize> + 'a {
        let c = &CLASSES[self.class as usize];
        c.chain
            .iter()
            .flat_map(|&l| CLASSES[l as usize].members.iter())
            .enumerate()
            .filter(move |(_, m)| m.tag == tag)
            .map(|(i, _)| i)
    }

    /// The first member with this element name.
    pub(crate) fn get(&self, tag: &str) -> Option<&Value> {
        let slot = self.slots(tag).next()?;
        self.members.get(slot)
    }

    pub(crate) fn get_mut(&mut self, tag: &str) -> Option<&mut Value> {
        let slot = self.slots(tag).next()?;
        self.members.get_mut(slot)
    }

    /// Text of the first member with this element name, when it has one.
    pub(crate) fn text(&self, tag: &str) -> Option<&str> {
        match self.get(tag)? {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The object held by the first member with this element name.
    pub(crate) fn child(&self, tag: &str) -> Option<&Node> {
        match self.get(tag)? {
            Value::Node(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn child_mut(&mut self, tag: &str) -> Option<&mut Node> {
        match self.get_mut(tag)? {
            Value::Node(n) => Some(n),
            _ => None,
        }
    }

    /// Set every member with this element name; false when there is none.
    pub(crate) fn set(&mut self, tag: &str, value: Value) -> bool {
        let slots: Vec<usize> = self.slots(tag).collect();
        for &slot in &slots {
            self.members[slot] = value.clone();
        }
        !slots.is_empty()
    }
}

/// The member at a position among all members of the class levels.
fn member_at(c: &Class, mut slot: usize) -> Option<&'static Member> {
    for &level in c.chain {
        let own = CLASSES[level as usize].members;
        if slot < own.len() {
            return Some(&own[slot]);
        }
        slot -= own.len();
    }
    None
}

fn fresh(def: Def, ty: Ty) -> Value {
    match def {
        Def::Null => Value::Null,
        Def::Str(s) => Value::Text(s.to_string()),
        Def::Int(i) => Value::Int(i),
        Def::Bool(b) => Value::Bool(b),
        Def::Enum(name) => Value::Enum { of: if let Ty::Enum(e) = ty { e } else { 0 }, name },
        Def::Opaque => Value::Opaque,
    }
}

/// The scanner parameters as an object model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Model {
    pub(crate) root: Node,
}

/// Marks an input the reference refuses, or a case the model does not
/// cover: nothing is produced.
#[derive(Debug)]
struct Refused;

/// Read the server's scanner parameters into the model; `None` when the
/// reference would refuse the text (ibx#571).
pub(crate) fn parse(server_xml: &str) -> Option<Model> {
    reader::read(server_xml).ok().filter(|(_, whole)| *whole).map(|(model, _)| model)
}

/// Write the model as the reference does. `None` when a member still
/// holds a default the model does not describe, or when an entry without
/// text sits where the reference has nothing to stop its failure
/// (ibx#571).
pub(crate) fn write(model: &Model) -> Option<String> {
    writer::write(model).ok()
}

/// The scanner parameters as the reference hands them to its clients:
/// the server text read into the model and written out again. `None`
/// when the reference would refuse the text (ibx#571).
pub fn rebuild(server_xml: &str) -> Option<String> {
    write(&parse(server_xml)?)
}

/// The scanner parameters as the reference hands them to a client of
/// this session: the server text read into the model, the items that
/// are not for a client or that the session does not have removed, the
/// model written out. `None` when the reference would refuse the text,
/// or when its answer depends on something the model does not have
/// (ibx#571).
///
/// It takes over 10 ms on the document of a real session: not for the
/// engine thread.
pub fn for_client(server_xml: &str, session: &Session) -> Option<String> {
    for_client_in(server_xml, session, server_year(session, jiff::Timestamp::now()))
}

/// The year of the years generator: the machine's clock `now` plus the
/// offset to the server's clock, in the machine's zone (ibx#653).
fn server_year(session: &Session, now: jiff::Timestamp) -> i32 {
    let server = now.checked_add(jiff::SignedDuration::from_millis(session.clock_offset_ms())).unwrap_or(now);
    i32::from(server.to_zoned(crate::gateway::machine_tz()).year())
}

/// [`for_client`] in a given year of the machine's clock: computed values
/// count the years up to it (ibx#599).
pub(crate) fn for_client_in(server_xml: &str, session: &Session, this_year: i32) -> Option<String> {
    // A text the reader stopped in is taken as far as it was read
    // (ibx#599).
    let (mut model, whole) = reader::read(server_xml).ok()?;
    session::apply(&mut model.root, session, this_year, whole).ok()?;
    write(&model)
}
