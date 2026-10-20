//! The FA data of an advisor session (ibx#507): the account groups, the
//! allocation profiles and the account aliases, as the reference 10.51
//! keeps them, hands them to `request_fa` and takes them from
//! `replace_fa`. Not observed yet: no FA account; the forms and the rules
//! follow what the reference is known to do.
//!
//! Two modes, as the logon says (feature `FAONEGRP`): the unified one,
//! where the profiles are groups whose accounts carry an amount, and the
//! classic one, with groups and profiles apart.

/// The logon feature of the unified mode.
pub const UNIFIED_FEATURE: &str = "FAONEGRP";
/// The logon feature that names the equal method `Equal`.
pub const EQUALPOS_FEATURE: &str = "EQUALPOS";
/// The logon feature that keeps the groups with no account.
pub const KEEP_EMPTY_FEATURE: &str = "CCPVLDACCT";

/// The text of `replace_fa_end`.
pub const SAVED: &str = "FA data saved";
/// Error 10229, a save that failed; the cause follows.
pub const SAVING_ERROR: &str = "FA data saving error";
/// The cause of 10229 when the link is lost before the answer.
pub const CONNECTION_FAILED: &str = "Connection failed";
/// The cause of 10229 for a classic import with a linked HF group.
pub const HF_LINKED: &str = "Contains HF linked groups";
/// Error 10228: an order on a group waited for a save that failed.
pub const ORDER_SAVE_FAILED: &str = "Group|Profile saving error";
/// Error 10230: `request_fa` while a save of that data is not answered.
pub const UNSAVED: &str = "You have unsaved FA changes. Please retry 'request FA' operation later, when 'replace FA' operation is complete";
/// Error 10262: the aliases are not replaced through the API.
pub const ALIAS_REFUSED: &str = "Replacing alias is not allowed. You can configure account aliases in the Account Management.";
/// Error 10258: a classic document in the unified mode.
pub const UNIFIED_FORMAT: &str = "Please use new import format for unified Account Groups with User-specified allocation.";
/// Error 10261.
pub const PARSE_ERROR: &str = "Error parsing XML";
/// Error 10259; the names follow.
pub const DUPLICATED: &str = "The following unified group names are duplicated:";
/// Error 10231; the names follow.
pub const INVALID_ACCOUNTS: &str = "The following Groups and/or Profiles contain invalid accounts:";

/// The 321 causes of the two requests.
pub const NO_SUCH_OPERATION: &str = "Non-existent FA data operation request.";
pub const XML_REQUIRED: &str = "An XML string is required for this operation.";

/// The profile types, by number (1 to 4).
const PROFILE_KINDS: [&str; 4] = ["Percent", "Ratio", "ContractsOrShares", "MonetaryAmount"];

/// An account group.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FaGroup {
    pub name: String,
    pub accounts: Vec<String>,
    pub method: Option<String>,
    pub risk: Option<String>,
    pub size: Option<String>,
    pub linked_hf: bool,
}

/// An allocation profile: an amount per account.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FaProfile {
    pub name: String,
    /// 1 Percent, 2 Ratio, 3 ContractsOrShares, 4 MonetaryAmount.
    pub kind: i32,
    pub allocations: Vec<(String, f64)>,
    pub risk: Option<String>,
}

/// The FA data of the session, and the saves not answered yet.
#[derive(Debug, Default)]
pub struct FaData {
    pub groups: Vec<FaGroup>,
    pub profiles: Vec<FaProfile>,
    /// (account, alias) of each account of the logon's list.
    pub aliases: Vec<(String, String)>,
    /// Saves sent and not answered, of the groups and of the profiles.
    pub pending_groups: u32,
    pub pending_profiles: u32,
}

impl FaData {
    /// The stored method of a group: `None` when there is no such group,
    /// `Some(None)` when it has none.
    pub fn group_method(&self, name: &str) -> Option<Option<&str>> {
        self.groups.iter().find(|g| g.name == name).map(|g| g.method.as_deref().filter(|m| !m.is_empty()))
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.iter().any(|p| p.name == name)
    }

    /// The group an order is allocated to: its group, or its account when
    /// that names a group, a profile, `All` or `AllNonProp`.
    pub fn allocation_group<'a>(&self, group: &'a str, account: &'a str) -> &'a str {
        if !group.is_empty() {
            return group;
        }
        if is_all(account) || self.groups.iter().any(|g| g.name == account) || self.has_profile(account) {
            return account;
        }
        ""
    }
}

/// One save to the server: delete the old objects of a kind, then save
/// the new ones. The answer of a replace comes when all the saves of a
/// batch are answered.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveBatch {
    /// `Group` or `Profile`: the server's type of the objects.
    pub kind: &'static str,
    /// True for the profiles (the pending count it holds).
    pub profiles: bool,
    pub deletes: Vec<String>,
    /// (name, document).
    pub saves: Vec<(String, String)>,
}

/// The pseudo groups of every account.
pub fn is_all(name: &str) -> bool {
    matches!(name, "All" | "AllNonProp")
}

/// Warning 2184: the first order on a group of an FA session.
pub const GROUP_WARNING: &str = "This order will be distributed over multiple accounts.<br>We strongly suggest you familiarize yourself \
    with our allocation<br> facilities at the link below before submitting orders.";

/// The method names of a group for the API (an import or an order).
pub fn group_methods(equalpos: bool) -> [&'static str; 5] {
    ["PctChange", "PctRoll", "AvailableEquity", "NetLiq", if equalpos { "Equal" } else { "EqualQuantity" }]
}

/// The (account, alias) list of the logon's account list: `acct` or
/// `acct/alias`; the alias is the account when none is given.
pub fn aliases(list: &str) -> Vec<(String, String)> {
    list.split(',')
        .filter_map(|item| {
            let mut it = item.splitn(2, '/');
            let account = it.next().unwrap_or("").trim();
            let alias = it.next().map(str::trim).filter(|a| !a.is_empty()).unwrap_or(account);
            (!account.is_empty()).then(|| (account.to_string(), alias.to_string()))
        })
        .collect()
}

// ── Writing ──

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// A double as the reference writes it (`50.0`, `33.5`).
fn double_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e7 { format!("{:.1}", v) } else { format!("{}", v) }
}

/// The document writer: one element a line, tabs for the depth.
struct Xml {
    out: String,
    depth: usize,
}

impl Xml {
    fn new() -> Self {
        Xml { out: HEADER.to_string(), depth: 0 }
    }
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push('\t');
        }
    }
    fn open(&mut self, name: &str, var_name: Option<&str>) {
        self.indent();
        self.out.push('<');
        self.out.push_str(name);
        if let Some(v) = var_name {
            self.out.push_str(" varName=\"");
            self.out.push_str(v);
            self.out.push('"');
        }
        self.out.push_str(">\n");
        self.depth += 1;
    }
    fn close(&mut self, name: &str) {
        self.depth -= 1;
        self.indent();
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }
    fn empty(&mut self, name: &str, var_name: Option<&str>) {
        self.indent();
        self.out.push('<');
        self.out.push_str(name);
        if let Some(v) = var_name {
            self.out.push_str(" varName=\"");
            self.out.push_str(v);
            self.out.push('"');
        }
        self.out.push_str("/>\n");
    }
    fn field(&mut self, name: &str, value: Option<&str>) {
        if let Some(v) = value {
            self.indent();
            self.out.push_str(&format!("<{}>{}</{}>\n", name, escape(v), name));
        }
    }
    /// A list element and its items, or the element alone when empty.
    fn list<T>(&mut self, name: &str, var_name: Option<&str>, items: &[T], mut item: impl FnMut(&mut Self, &T)) {
        if items.is_empty() {
            self.empty(name, var_name);
            return;
        }
        self.open(name, var_name);
        for i in items {
            item(self, i);
        }
        self.close(name);
    }
}

fn kind_name(kind: i32) -> Option<&'static str> {
    usize::try_from(kind - 1).ok().and_then(|i| PROFILE_KINDS.get(i).copied())
}

fn profile_kind(method: &str) -> Option<i32> {
    PROFILE_KINDS.iter().position(|k| *k == method).map(|i| i as i32 + 1)
}

/// A classic group (the server's form, and the classic answer).
fn write_classic_group(x: &mut Xml, g: &FaGroup) {
    x.open("Group", None);
    x.field("name", Some(&g.name));
    x.list("ListOfAccts", Some("list"), &g.accounts, |x, a| x.field("String", Some(a)));
    x.field("defaultMethod", g.method.as_deref());
    x.field("riskCriteria", g.risk.as_deref());
    x.field("linkedHFGroup", Some(if g.linked_hf { "true" } else { "false" }));
    x.field("defaultSize", g.size.as_deref());
    x.close("Group");
}

/// A profile saved as a group (the unified mode): its allocations, its
/// type as the method.
fn write_profile_as_group(x: &mut Xml, p: &FaProfile) {
    x.open("Group", None);
    x.field("name", Some(&p.name));
    x.list("ListOfAllocations", Some("listOfAllocations"), &p.allocations, write_allocation);
    x.field("defaultMethod", kind_name(p.kind));
    x.field("riskCriteria", p.risk.as_deref());
    x.field("linkedHFGroup", Some("false"));
    x.close("Group");
}

fn write_allocation(x: &mut Xml, (acct, amount): &(String, f64)) {
    x.open("Allocation", None);
    x.field("acct", Some(acct));
    x.field("amount", Some(&double_text(*amount)));
    x.close("Allocation");
}

fn write_profile(x: &mut Xml, p: &FaProfile) {
    x.open("AllocationProfile", None);
    x.field("name", Some(&p.name));
    x.field("type", Some(&p.kind.to_string()));
    x.list("ListOfAllocations", Some("listOfAllocations"), &p.allocations, write_allocation);
    x.field("riskCriteria", p.risk.as_deref());
    x.close("AllocationProfile");
}

/// The answer of `request_fa` for the groups: in the unified mode the
/// profiles then the groups, each account with its amount for a profile;
/// in the classic mode the groups.
pub fn groups_xml(data: &FaData, unified: bool) -> String {
    let mut x = Xml::new();
    if !unified {
        x.list("ListOfGroups", None, &data.groups, write_classic_group);
        return x.out;
    }
    enum Item<'a> { Profile(&'a FaProfile), Group(&'a FaGroup) }
    let items: Vec<Item> = data.profiles.iter().map(Item::Profile).chain(data.groups.iter().map(Item::Group)).collect();
    x.list("ListOfGroups", None, &items, |x, item| {
        x.open("Group", None);
        match item {
            Item::Profile(p) => {
                x.field("name", Some(&p.name));
                x.field("defaultMethod", kind_name(p.kind));
                x.list("ListOfAccts", Some("list"), &p.allocations, |x, (acct, amount)| {
                    x.open("Account", None);
                    x.field("acct", Some(acct));
                    x.field("amount", Some(&double_text(*amount)));
                    x.close("Account");
                });
                x.field("riskCriteria", p.risk.as_deref());
            }
            Item::Group(g) => {
                x.field("name", Some(&g.name));
                x.field("defaultMethod", g.method.as_deref());
                x.list("ListOfAccts", Some("list"), &g.accounts, |x, acct| {
                    x.open("Account", None);
                    x.field("acct", Some(acct));
                    x.close("Account");
                });
                x.field("riskCriteria", g.risk.as_deref());
                x.field("defaultSize", g.size.as_deref());
            }
        }
        x.close("Group");
    });
    x.out
}

/// The answer of `request_fa` for the aliases.
pub fn aliases_xml(aliases: &[(String, String)]) -> String {
    let mut x = Xml::new();
    x.list("ListOfAccountAliases", None, aliases, |x, (account, alias)| {
        x.open("AccountAlias", None);
        x.field("account", Some(account));
        x.field("alias", Some(alias));
        x.close("AccountAlias");
    });
    x.out
}

/// The document of one group saved to the server.
pub fn group_document(g: &FaGroup) -> String {
    let mut x = Xml::new();
    write_classic_group(&mut x, g);
    x.out
}

/// The document of one profile saved to the server: a group in the
/// unified mode, a profile otherwise.
pub fn profile_document(p: &FaProfile, unified: bool) -> String {
    let mut x = Xml::new();
    if unified { write_profile_as_group(&mut x, p) } else { write_profile(&mut x, p) }
    x.out
}

// ── Reading ──

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Node, name: &str) -> Option<String> {
    child(n, name).map(|c| c.text().unwrap_or("").to_string())
}

fn elements<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The document without the spaces between its elements, as the
/// reference takes it.
pub fn clean(xml: &str) -> String {
    // The spaces after a `>`.
    let mut after = String::with_capacity(xml.len());
    let mut skip = false;
    for c in xml.chars() {
        if skip && c.is_ascii_whitespace() {
            continue;
        }
        skip = c == '>';
        after.push(c);
    }
    // The spaces before a `<`.
    let mut out = String::with_capacity(after.len());
    let mut run = String::new();
    for c in after.chars() {
        if c.is_ascii_whitespace() {
            run.push(c);
            continue;
        }
        if c != '<' {
            out.push_str(&run);
        }
        run.clear();
        out.push(c);
    }
    out.push_str(&run);
    out
}

/// True for a document of the unified form: no `<AllocationProfile` and
/// no `<String>`.
pub fn unified_format(xml: &str) -> bool {
    !xml.contains("<AllocationProfile") && !xml.contains("<String>")
}

fn parse(xml: &str) -> Option<roxmltree::Document<'_>> {
    roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).ok()
}

/// A classic group of a document, with the allocations of a profile
/// saved as a group.
struct ClassicGroup {
    group: FaGroup,
    allocations: Option<Vec<(String, f64)>>,
}

fn read_allocations(list: Node) -> Vec<(String, f64)> {
    elements(list, "Allocation")
        .map(|a| (text_of(a, "acct").unwrap_or_default(), text_of(a, "amount").and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)))
        .collect()
}

fn read_classic_groups(xml: &str) -> Option<Vec<ClassicGroup>> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    if root.tag_name().name() != "ListOfGroups" {
        return None;
    }
    Some(elements(root, "Group").map(|g| ClassicGroup {
        group: FaGroup {
            name: text_of(g, "name").unwrap_or_default(),
            accounts: child(g, "ListOfAccts").map(|l| elements(l, "String").map(|s| s.text().unwrap_or("").to_string()).collect()).unwrap_or_default(),
            method: text_of(g, "defaultMethod"),
            risk: text_of(g, "riskCriteria"),
            size: text_of(g, "defaultSize"),
            linked_hf: text_of(g, "linkedHFGroup").is_some_and(|v| v.trim() == "true"),
        },
        allocations: child(g, "ListOfAllocations").map(read_allocations),
    }).collect())
}

/// The groups the server gives at the logon (its "get all" answer). In
/// the unified mode a group that carries allocations is a profile. The
/// groups named in `linked_hf` are linked HF groups. A group with no
/// account is left out unless `keep_empty`.
pub fn server_groups(xml: &str, unified: bool, linked_hf: &[&str], keep_empty: bool) -> (Vec<FaGroup>, Vec<FaProfile>) {
    let Some(read) = read_classic_groups(&clean(xml)) else {
        log::warn!("FA groups of the server not read");
        return (Vec::new(), Vec::new());
    };
    let mut groups = Vec::new();
    let mut profiles = Vec::new();
    for ClassicGroup { mut group, allocations } in read {
        match allocations {
            Some(allocations) if unified => profiles.push(FaProfile {
                kind: group.method.as_deref().and_then(profile_kind).unwrap_or(1),
                name: group.name, allocations, risk: group.risk,
            }),
            _ => {
                group.linked_hf |= linked_hf.contains(&group.name.as_str());
                if keep_empty || !group.accounts.is_empty() {
                    groups.push(group);
                }
            }
        }
    }
    (groups, profiles)
}

/// The profiles the server gives at the logon in the classic mode.
pub fn server_profiles(xml: &str) -> Vec<FaProfile> {
    let xml = clean(xml);
    let Some(doc) = parse(&xml) else {
        log::warn!("FA profiles of the server not read");
        return Vec::new();
    };
    elements(doc.root_element(), "AllocationProfile").map(|p| FaProfile {
        name: text_of(p, "name").unwrap_or_default(),
        kind: text_of(p, "type").and_then(|t| t.trim().parse().ok()).unwrap_or(1),
        allocations: child(p, "ListOfAllocations").map(read_allocations).unwrap_or_default(),
        risk: text_of(p, "riskCriteria"),
    }).collect()
}

// ── Replacing ──

/// The session's facts a replace needs.
pub struct Session<'a> {
    pub unified: bool,
    pub equalpos: bool,
    pub keep_empty: bool,
    /// The accounts of the logon's list.
    pub accounts: &'a [String],
}

/// What a replace of the groups does: the new data, and the saves.
#[derive(Debug)]
pub struct Replace {
    pub groups: Vec<FaGroup>,
    pub profiles: Vec<FaProfile>,
    pub batches: Vec<SaveBatch>,
}

/// A replace refused: the error (code and text), or nothing at all, as
/// the reference gives nothing for a document it cannot read in the
/// unified mode or one with nothing left in it.
pub type Refusal = Option<(i64, String)>;

/// The 321 cause of a replace, before anything is read: a data type that
/// does not exist (the profiles are not given apart at this session's
/// level), or no document.
pub fn replace_cause(fa_data_type: i32, xml: &str) -> Option<&'static str> {
    if !(1..=3).contains(&fa_data_type) || fa_data_type == 2 {
        return Some(NO_SUCH_OPERATION);
    }
    xml.is_empty().then_some(XML_REQUIRED)
}

/// The 321 cause of a request: a data type that does not exist, the
/// profiles included at this session's level.
pub fn request_cause(fa_data_type: i32) -> Option<&'static str> {
    (!matches!(fa_data_type, 1 | 3)).then_some(NO_SUCH_OPERATION)
}

fn names_error(code: i64, text: &str, names: &[String]) -> (i64, String) {
    (code, crate::control::historical::join_error_text(text, &names.join(", ")))
}

/// A replace of the data of type `fa_data_type` (1 groups, 3 aliases;
/// `replace_cause` passed) with `xml`, against the data `old`.
pub fn replace(fa_data_type: i32, xml: &str, old: &FaData, s: &Session) -> Result<Replace, Refusal> {
    if fa_data_type == 3 {
        return Err(Some((10262, ALIAS_REFUSED.into())));
    }
    let xml = clean(xml);
    if s.unified && !unified_format(&xml) {
        return Err(Some((10258, UNIFIED_FORMAT.into())));
    }
    let (groups, profiles) = if s.unified { read_unified(&xml, s)? } else { read_classic(&xml)? };
    let (groups, profiles) = keep_valid(groups, profiles, s)?;
    let mut batches = Vec::new();
    batches.push(SaveBatch {
        kind: "Group", profiles: false,
        deletes: old.groups.iter().map(|g| g.name.clone()).collect(),
        saves: groups.iter().map(|g| (g.name.clone(), group_document(g))).collect(),
    });
    if s.unified {
        batches.push(SaveBatch {
            kind: "Group", profiles: true,
            deletes: old.profiles.iter().map(|p| p.name.clone()).collect(),
            saves: profiles.iter().map(|p| (p.name.clone(), profile_document(p, true))).collect(),
        });
    }
    Ok(Replace { groups, profiles: if s.unified { profiles } else { old.profiles.clone() }, batches })
}

/// A unified document: its groups, and its profiles (the groups whose
/// accounts all carry an amount).
fn read_unified(xml: &str, s: &Session) -> Result<(Vec<FaGroup>, Vec<FaProfile>), Refusal> {
    let Some(doc) = parse(xml) else {
        log::warn!("replace_fa: the unified groups are not read");
        return Err(None);
    };
    let root = doc.root_element();
    if root.tag_name().name() != "ListOfGroups" {
        log::warn!("replace_fa: the unified groups are not read");
        return Err(None);
    }
    struct Read { name: String, method: Option<String>, accounts: Vec<(String, Option<String>)>, risk: Option<String>, size: Option<String> }
    let read: Vec<Read> = elements(root, "Group").map(|g| Read {
        name: text_of(g, "name").unwrap_or_default(),
        method: text_of(g, "defaultMethod"),
        accounts: child(g, "ListOfAccts").map(|l| elements(l, "Account")
            .map(|a| (text_of(a, "acct").unwrap_or_default(), text_of(a, "amount").filter(|v| !v.is_empty())))
            .collect()).unwrap_or_default(),
        risk: text_of(g, "riskCriteria"),
        size: text_of(g, "defaultSize"),
    }).collect();
    if read.is_empty() {
        return Err(Some((10261, PARSE_ERROR.into())));
    }
    let mut seen = std::collections::HashSet::new();
    let mut duplicated: Vec<String> = Vec::new();
    for r in &read {
        if !seen.insert(r.name.as_str()) && !duplicated.contains(&r.name) {
            duplicated.push(r.name.clone());
        }
    }
    if !duplicated.is_empty() {
        return Err(Some(names_error(10259, DUPLICATED, &duplicated)));
    }
    let is_profile = |r: &Read| !r.accounts.is_empty() && r.accounts.iter().all(|(a, amount)| !a.is_empty() && amount.is_some());
    let unsupported = |r: &Read| (10260, format!("Group {} has unsupported method ({})", r.name, r.method.as_deref().unwrap_or("null")));
    for r in read.iter().filter(|r| is_profile(r)) {
        if r.method.as_deref().and_then(profile_kind).is_none() {
            return Err(Some(unsupported(r)));
        }
    }
    let methods = group_methods(s.equalpos);
    // An API import does not take PctChange as a group's method.
    for r in read.iter().filter(|r| !is_profile(r)) {
        if !r.method.as_deref().is_some_and(|m| m != "PctChange" && methods.contains(&m)) {
            return Err(Some(unsupported(r)));
        }
    }
    let mut groups = Vec::new();
    let mut profiles = Vec::new();
    for r in read {
        if is_profile(&r) {
            profiles.push(FaProfile {
                kind: r.method.as_deref().and_then(profile_kind).unwrap_or(1),
                allocations: r.accounts.into_iter().map(|(a, amount)| (a, amount.and_then(|v| v.trim().parse().ok()).unwrap_or(0.0))).collect(),
                name: r.name, risk: r.risk,
            });
        } else {
            groups.push(FaGroup {
                accounts: r.accounts.into_iter().map(|(a, _)| a).collect(),
                name: r.name, method: r.method, risk: r.risk, size: r.size, linked_hf: false,
            });
        }
    }
    if groups.is_empty() && profiles.is_empty() {
        return Err(None);
    }
    Ok((groups, profiles))
}

/// A classic document of groups: refused with 10229 when a group is a
/// linked HF group or it does not read, with 10261 for a unified one.
fn read_classic(xml: &str) -> Result<(Vec<FaGroup>, Vec<FaProfile>), Refusal> {
    let read = read_classic_groups(xml);
    if read.as_ref().is_none_or(|r| r.iter().any(|g| g.group.linked_hf)) {
        return Err(Some((10229, crate::control::historical::join_error_text(SAVING_ERROR, HF_LINKED))));
    }
    if unified_format(xml) {
        return Err(Some((10261, PARSE_ERROR.into())));
    }
    Ok((read.unwrap_or_default().into_iter().map(|g| g.group).collect(), Vec::new()))
}

/// The groups with no account left out (unless the logon keeps them);
/// 10231 when a group or a profile has an account outside the logon's
/// list.
fn keep_valid(mut groups: Vec<FaGroup>, profiles: Vec<FaProfile>, s: &Session) -> Result<(Vec<FaGroup>, Vec<FaProfile>), Refusal> {
    if !s.keep_empty {
        groups.retain(|g| !g.accounts.is_empty());
    }
    let known = |a: &String| s.accounts.contains(a);
    let bad: Vec<String> = groups.iter().filter(|g| !g.accounts.iter().all(known)).map(|g| g.name.clone()).collect();
    if !bad.is_empty() {
        return Err(Some(names_error(10231, INVALID_ACCOUNTS, &bad)));
    }
    let bad: Vec<String> = profiles.iter().filter(|p| !p.allocations.iter().all(|(a, _)| known(a))).map(|p| p.name.clone()).collect();
    if !bad.is_empty() {
        return Err(Some(names_error(10231, INVALID_ACCOUNTS, &bad)));
    }
    Ok((groups, profiles))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(unified: bool, accounts: &[String]) -> Session<'_> {
        Session { unified, equalpos: false, keep_empty: false, accounts }
    }

    #[test]
    fn the_server_groups_and_the_answers() {
        let server = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListOfGroups>\n\t<Group>\n\t\t<name>G1</name>\n\
            \t\t<ListOfAccts varName=\"list\">\n\t\t\t<String>U1</String>\n\t\t\t<String>U2</String>\n\t\t</ListOfAccts>\n\
            \t\t<defaultMethod>NetLiq</defaultMethod>\n\t</Group>\n\t<Group>\n\t\t<name>P1</name>\n\
            \t\t<ListOfAllocations varName=\"listOfAllocations\">\n\t\t\t<Allocation>\n\t\t\t\t<acct>U1</acct>\n\
            \t\t\t\t<amount>60.0</amount>\n\t\t\t</Allocation>\n\t\t</ListOfAllocations>\n\
            \t\t<defaultMethod>Percent</defaultMethod>\n\t</Group>\n\t<Group>\n\t\t<name>E</name>\n\t</Group>\n</ListOfGroups>\n";
        let (groups, profiles) = server_groups(server, true, &["G1"], false);
        assert_eq!(groups.len(), 1, "the empty group is left out");
        assert_eq!(groups[0].accounts, ["U1", "U2"]);
        assert!(groups[0].linked_hf);
        assert_eq!(profiles, [FaProfile { name: "P1".into(), kind: 1, allocations: vec![("U1".into(), 60.0)], risk: None }]);
        let data = FaData { groups, profiles, ..Default::default() };
        assert_eq!(groups_xml(&data, true), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListOfGroups>\n\
            \t<Group>\n\t\t<name>P1</name>\n\t\t<defaultMethod>Percent</defaultMethod>\n\t\t<ListOfAccts varName=\"list\">\n\
            \t\t\t<Account>\n\t\t\t\t<acct>U1</acct>\n\t\t\t\t<amount>60.0</amount>\n\t\t\t</Account>\n\t\t</ListOfAccts>\n\t</Group>\n\
            \t<Group>\n\t\t<name>G1</name>\n\t\t<defaultMethod>NetLiq</defaultMethod>\n\t\t<ListOfAccts varName=\"list\">\n\
            \t\t\t<Account>\n\t\t\t\t<acct>U1</acct>\n\t\t\t</Account>\n\t\t\t<Account>\n\t\t\t\t<acct>U2</acct>\n\t\t\t</Account>\n\
            \t\t</ListOfAccts>\n\t</Group>\n</ListOfGroups>\n");
        // In the classic mode a group with allocations is a group: with no
        // account, left out.
        let (groups, profiles) = server_groups(server, false, &[], false);
        assert_eq!((groups.len(), profiles.len()), (1, 0));
        assert_eq!(server_groups(server, false, &[], true).0.len(), 3, "kept with CCPVLDACCT");
        assert_eq!(groups_xml(&FaData::default(), true), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListOfGroups/>\n");
        assert_eq!(aliases_xml(&aliases("U1/Main, U2")), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListOfAccountAliases>\n\
            \t<AccountAlias>\n\t\t<account>U1</account>\n\t\t<alias>Main</alias>\n\t</AccountAlias>\n\
            \t<AccountAlias>\n\t\t<account>U2</account>\n\t\t<alias>U2</alias>\n\t</AccountAlias>\n</ListOfAccountAliases>\n");
    }

    #[test]
    fn a_replace_reads_checks_and_saves() {
        let accounts = vec!["U1".to_string(), "U2".to_string()];
        let s = session(true, &accounts);
        let old = FaData { groups: vec![FaGroup { name: "Old".into(), accounts: vec!["U1".into()], ..Default::default() }], ..Default::default() };
        let doc = "<ListOfGroups>\n  <Group><name>G</name><defaultMethod>NetLiq</defaultMethod>\
            <ListOfAccts varName=\"list\"><Account><acct>U1</acct></Account></ListOfAccts></Group>\
            <Group><name>P</name><defaultMethod>Ratio</defaultMethod>\
            <ListOfAccts varName=\"list\"><Account><acct>U2</acct><amount>2</amount></Account></ListOfAccts></Group>\
            <Group><name>Empty</name><defaultMethod>NetLiq</defaultMethod></Group></ListOfGroups>";
        let r = replace(1, doc, &old, &s).unwrap();
        assert_eq!(r.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["G"], "the empty group goes");
        assert_eq!(r.profiles, [FaProfile { name: "P".into(), kind: 2, allocations: vec![("U2".into(), 2.0)], risk: None }]);
        assert_eq!(r.batches[0].deletes, ["Old"]);
        assert_eq!(r.batches[0].saves[0].0, "G");
        assert!(r.batches[0].saves[0].1.contains("<String>U1</String>"));
        assert_eq!((r.batches[1].kind, r.batches[1].profiles), ("Group", true));
        assert!(r.batches[1].saves[0].1.contains("<amount>2.0</amount>"), "{}", r.batches[1].saves[0].1);

        let refused = |doc: &str, s: &Session| replace(1, doc, &old, s).unwrap_err();
        assert_eq!(replace(3, doc, &old, &s).unwrap_err(), Some((10262, ALIAS_REFUSED.into())));
        assert_eq!(refused("<ListOfGroups><Group><String>U1</String></Group></ListOfGroups>", &s), Some((10258, UNIFIED_FORMAT.into())));
        assert_eq!(refused("<ListOfGroups/>", &s), Some((10261, PARSE_ERROR.into())));
        assert_eq!(refused("<ListOfGroups><Group>", &s), None, "not read: no answer");
        assert_eq!(refused("<ListOfGroups><Group><name>A</name></Group><Group><name>A</name></Group></ListOfGroups>", &s),
            Some((10259, "The following unified group names are duplicated:A".into())));
        assert_eq!(refused("<ListOfGroups><Group><name>A</name><defaultMethod>PctChange</defaultMethod></Group></ListOfGroups>", &s),
            Some((10260, "Group A has unsupported method (PctChange)".into())));
        assert_eq!(refused("<ListOfGroups><Group><name>A</name><defaultMethod>NetLiq</defaultMethod><ListOfAccts>\
            <Account><acct>U9</acct></Account></ListOfAccts></Group></ListOfGroups>", &s),
            Some((10231, "The following Groups and/or Profiles contain invalid accounts:A".into())));
        // Nothing left once the empty groups go: the old ones are deleted,
        // nothing is saved, and no answer comes.
        let r = replace(1, "<ListOfGroups><Group><name>A</name><defaultMethod>NetLiq</defaultMethod></Group></ListOfGroups>", &old, &s).unwrap();
        assert!(r.groups.is_empty() && r.batches.iter().all(|b| b.saves.is_empty()));
        assert_eq!(r.batches[0].deletes, ["Old"]);

        // The classic mode.
        let c = session(false, &accounts);
        let classic = "<ListOfGroups><Group><name>A</name><ListOfAccts varName=\"list\"><String>U1</String></ListOfAccts>\
            <defaultMethod>NetLiq</defaultMethod><linkedHFGroup>false</linkedHFGroup></Group></ListOfGroups>";
        let r = replace(1, classic, &old, &c).unwrap();
        assert_eq!((r.groups.len(), r.batches.len()), (1, 1));
        assert_eq!(refused(&classic.replace(">false<", ">true<"), &c), Some((10229, "FA data saving error:Contains HF linked groups".into())));
        assert_eq!(refused("<ListOfGroups/>", &c), Some((10261, PARSE_ERROR.into())));

        assert_eq!(replace_cause(2, doc), Some(NO_SUCH_OPERATION));
        assert_eq!(replace_cause(1, ""), Some(XML_REQUIRED));
        assert_eq!(request_cause(2), Some(NO_SUCH_OPERATION));
        assert_eq!(request_cause(3), None);
    }

    #[test]
    fn clean_drops_the_spaces_between_elements() {
        assert_eq!(clean("  <a>\n\t<b> x y </b>\n</a>  "), "<a><b>x y</b></a>");
    }
}
