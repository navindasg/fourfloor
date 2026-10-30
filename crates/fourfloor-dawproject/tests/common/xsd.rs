//! A small structural XML Schema checker, enough for the constructs the
//! DAWproject schemas use: global and local elements, `ref`, named and
//! anonymous complex types, `complexContent/extension`, `sequence` and
//! `choice` with occurrence bounds, attributes (`use="required"`), simple
//! types by enumeration and list, and the built-in types `string`,
//! `double`, `int`, `boolean`, `ID` and `IDREF`.
//!
//! Content models are matched greedily without backtracking, which is
//! exact for the deterministic models XSD's Unique Particle Attribution
//! rule demands. It is not a general validator; `xmllint` is the full check.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use roxmltree::{Document, Node};

#[derive(Clone, Debug)]
enum Simple {
    Builtin(String),
    Enum(Vec<String>),
    List(Box<Simple>),
}

#[derive(Clone, Debug)]
enum Type {
    Complex(String),
    AnonComplex(Box<Complex>),
    Simple(Simple),
}

#[derive(Clone, Debug)]
struct ElementDecl {
    name: String,
    ty: Type,
}

#[derive(Clone, Debug)]
enum Particle {
    Element(ElementDecl, u32, u32),
    Sequence(Vec<Particle>, u32, u32),
    Choice(Vec<Particle>, u32, u32),
}

#[derive(Clone, Debug)]
struct Attr {
    name: String,
    ty: Simple,
    required: bool,
}

#[derive(Clone, Debug, Default)]
struct Complex {
    base: Option<String>,
    is_abstract: bool,
    content: Vec<Particle>,
    attrs: Vec<Attr>,
}

/// A loaded schema.
pub struct Schema {
    elements: HashMap<String, ElementDecl>,
    complex: HashMap<String, Complex>,
    simple: HashMap<String, Simple>,
}

fn xs_children<'a, 'i>(node: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(Node::is_element)
}

fn local(node: Node) -> &'static str {
    // Leaking is fine in a test: the names are a small fixed set.
    Box::leak(node.tag_name().name().to_owned().into_boxed_str())
}

fn occurs(node: Node, name: &str) -> u32 {
    match node.attribute(name) {
        Some("unbounded") => u32::MAX,
        Some(n) => n.parse().expect("occurrence bound"),
        None => 1,
    }
}

fn strip(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

impl Schema {
    /// Parses an `.xsd` file.
    pub fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).expect("read schema");
        let doc = Document::parse(&text).expect("parse schema");
        let mut schema = Schema {
            elements: HashMap::new(),
            complex: HashMap::new(),
            simple: HashMap::new(),
        };
        // Simple and complex types first, then elements (which may
        // reference anything).
        for node in xs_children(doc.root_element()) {
            if local(node) == "simpleType" {
                let name = node.attribute("name").expect("named simple type");
                let ty = schema.simple_type(node);
                schema.simple.insert(name.to_owned(), ty);
            }
        }
        for node in xs_children(doc.root_element()) {
            if local(node) == "complexType" {
                let name = node.attribute("name").expect("named complex type");
                let ty = schema.complex_type(node);
                schema.complex.insert(name.to_owned(), ty);
            }
        }
        for node in xs_children(doc.root_element()) {
            if local(node) == "element" {
                let decl = schema.element_decl(node);
                schema.elements.insert(decl.name.clone(), decl);
            }
        }
        schema
    }

    fn simple_type(&self, node: Node) -> Simple {
        for child in xs_children(node) {
            match local(child) {
                "restriction" => {
                    let values: Vec<String> = xs_children(child)
                        .filter(|n| local(*n) == "enumeration")
                        .filter_map(|n| n.attribute("value").map(str::to_owned))
                        .collect();
                    if !values.is_empty() {
                        return Simple::Enum(values);
                    }
                    let base = child.attribute("base").unwrap_or("xs:string");
                    return Simple::Builtin(strip(base).to_owned());
                }
                "list" => {
                    let item = child.attribute("itemType").unwrap_or("xs:string");
                    return Simple::List(Box::new(self.simple_named(item)));
                }
                _ => {}
            }
        }
        Simple::Builtin("string".to_owned())
    }

    fn simple_named(&self, name: &str) -> Simple {
        if name.starts_with("xs:") {
            Simple::Builtin(strip(name).to_owned())
        } else {
            self.simple
                .get(name)
                .cloned()
                .unwrap_or_else(|| panic!("unknown simple type {name}"))
        }
    }

    fn type_named(&self, name: &str) -> Type {
        if name.starts_with("xs:") || self.simple.contains_key(name) {
            Type::Simple(self.simple_named(name))
        } else {
            Type::Complex(name.to_owned())
        }
    }

    fn element_decl(&self, node: Node) -> ElementDecl {
        let name = node.attribute("name").expect("element name").to_owned();
        let ty = if let Some(ty) = node.attribute("type") {
            self.type_named(ty)
        } else if let Some(inner) = xs_children(node).find(|n| local(*n) == "complexType") {
            Type::AnonComplex(Box::new(self.complex_type(inner)))
        } else if let Some(inner) = xs_children(node).find(|n| local(*n) == "simpleType") {
            Type::Simple(self.simple_type(inner))
        } else {
            Type::Simple(Simple::Builtin("anyType".to_owned()))
        };
        ElementDecl { name, ty }
    }

    fn complex_type(&self, node: Node) -> Complex {
        let mut out = Complex {
            is_abstract: node.attribute("abstract") == Some("true"),
            ..Complex::default()
        };
        self.complex_body(node, &mut out);
        out
    }

    fn complex_body(&self, node: Node, out: &mut Complex) {
        for child in xs_children(node) {
            match local(child) {
                "complexContent" => {
                    for ext in xs_children(child) {
                        out.base = ext.attribute("base").map(str::to_owned);
                        self.complex_body(ext, out);
                    }
                }
                "sequence" | "choice" => out.content.push(self.particle(child)),
                "attribute" => out.attrs.push(self.attr(child)),
                _ => {}
            }
        }
    }

    fn attr(&self, node: Node) -> Attr {
        let ty = match node.attribute("type") {
            Some(ty) => self.simple_named(ty),
            None => xs_children(node)
                .find(|n| local(*n) == "simpleType")
                .map_or(Simple::Builtin("string".to_owned()), |n| {
                    self.simple_type(n)
                }),
        };
        Attr {
            name: node.attribute("name").expect("attribute name").to_owned(),
            ty,
            required: node.attribute("use") == Some("required"),
        }
    }

    fn particle(&self, node: Node) -> Particle {
        let (min, max) = (occurs(node, "minOccurs"), occurs(node, "maxOccurs"));
        match local(node) {
            "element" => {
                // Refs are resolved lazily (by name) at check time, since
                // global elements are loaded after the types.
                let decl = match node.attribute("ref") {
                    Some(name) => ElementDecl {
                        name: name.to_owned(),
                        ty: Type::Complex(format!("@ref:{name}")),
                    },
                    None => self.element_decl(node),
                };
                Particle::Element(decl, min, max)
            }
            "sequence" => Particle::Sequence(
                xs_children(node).map(|n| self.particle(n)).collect(),
                min,
                max,
            ),
            "choice" => Particle::Choice(
                xs_children(node).map(|n| self.particle(n)).collect(),
                min,
                max,
            ),
            other => panic!("unsupported particle xs:{other}"),
        }
    }

    fn resolve<'s>(&'s self, decl: &'s ElementDecl) -> &'s ElementDecl {
        match &decl.ty {
            Type::Complex(name) if name.starts_with("@ref:") => self
                .elements
                .get(&decl.name)
                .unwrap_or_else(|| panic!("unknown element ref {}", decl.name)),
            _ => decl,
        }
    }

    /// The content particles and attributes of a complex type, base first.
    fn flatten<'s>(
        &'s self,
        ty: &'s Complex,
        parts: &mut Vec<&'s Particle>,
        attrs: &mut Vec<&'s Attr>,
    ) {
        if let Some(base) = &ty.base {
            let base = self
                .complex
                .get(base)
                .unwrap_or_else(|| panic!("unknown base type {base}"));
            self.flatten(base, parts, attrs);
        }
        parts.extend(ty.content.iter());
        attrs.extend(ty.attrs.iter());
    }

    /// Checks `xml` against the schema, starting at global element `root`.
    /// Returns one message per problem; empty means it passed.
    pub fn check(&self, xml: &str, root: &str) -> Vec<String> {
        let doc = match Document::parse(xml) {
            Ok(doc) => doc,
            Err(error) => return vec![format!("not well-formed: {error}")],
        };
        let mut state = State::default();
        let element = doc.root_element();
        match self.elements.get(root) {
            Some(decl) if element.tag_name().name() == root => {
                self.check_element(element, decl, "", &mut state);
            }
            _ => state.errors.push(format!(
                "root is <{}>, expected <{root}>",
                element.tag_name().name()
            )),
        }
        for (path, idref) in &state.idrefs {
            if !state.ids.contains(idref) {
                state
                    .errors
                    .push(format!("{path}: IDREF `{idref}` names no element"));
            }
        }
        state.errors
    }

    fn check_element(&self, node: Node, decl: &ElementDecl, parent: &str, state: &mut State) {
        let path = format!("{parent}/{}", decl.name);
        match &decl.ty {
            Type::Simple(simple) => {
                if node.children().any(|c| c.is_element()) {
                    state
                        .errors
                        .push(format!("{path}: simple content has child elements"));
                }
                let text = node.text().unwrap_or("");
                if let Err(e) = check_simple(simple, text) {
                    state.errors.push(format!("{path}: {e}"));
                }
            }
            Type::Complex(name) => {
                let ty = self
                    .complex
                    .get(name)
                    .unwrap_or_else(|| panic!("unknown complex type {name}"));
                self.check_complex(node, ty, &path, state);
            }
            Type::AnonComplex(ty) => self.check_complex(node, ty, &path, state),
        }
    }

    fn check_complex(&self, node: Node, ty: &Complex, path: &str, state: &mut State) {
        if ty.is_abstract {
            state
                .errors
                .push(format!("{path}: element of an abstract type"));
        }
        let mut parts = Vec::new();
        let mut attrs = Vec::new();
        self.flatten(ty, &mut parts, &mut attrs);

        for attr in node.attributes() {
            if attr.namespace().is_some() {
                continue;
            }
            match attrs.iter().find(|a| a.name == attr.name()) {
                None => state
                    .errors
                    .push(format!("{path}: undeclared attribute `{}`", attr.name())),
                Some(decl) => {
                    if let Err(e) = check_simple(&decl.ty, attr.value()) {
                        state.errors.push(format!("{path}@{}: {e}", attr.name()));
                    }
                    match &decl.ty {
                        Simple::Builtin(b)
                            if b == "ID" && !state.ids.insert(attr.value().to_owned()) =>
                        {
                            state
                                .errors
                                .push(format!("{path}: duplicate id `{}`", attr.value()));
                        }
                        Simple::Builtin(b) if b == "IDREF" => state
                            .idrefs
                            .push((format!("{path}@{}", attr.name()), attr.value().to_owned())),
                        _ => {}
                    }
                }
            }
        }
        for decl in &attrs {
            if decl.required && node.attribute(decl.name.as_str()).is_none() {
                state.errors.push(format!(
                    "{path}: missing required attribute `{}`",
                    decl.name
                ));
            }
        }
        if node
            .children()
            .any(|c| c.is_text() && !c.text().unwrap_or("").trim().is_empty())
        {
            state
                .errors
                .push(format!("{path}: text inside element-only content"));
        }

        let children: Vec<Node> = node.children().filter(Node::is_element).collect();
        let mut matched: Vec<Option<&ElementDecl>> = vec![None; children.len()];
        let mut pos = 0;
        for part in &parts {
            match self.match_particle(part, &children, pos, &mut matched) {
                Some(next) => pos = next,
                None => {
                    state.errors.push(format!(
                        "{path}: children do not fit the content model near child {pos} ({})",
                        names(&children[pos.min(children.len())..])
                    ));
                    return;
                }
            }
        }
        if pos < children.len() {
            state.errors.push(format!(
                "{path}: unexpected children from position {pos}: {}",
                names(&children[pos..])
            ));
            return;
        }
        for (child, decl) in children.iter().zip(matched) {
            if let Some(decl) = decl {
                self.check_element(*child, decl, path, state);
            }
        }
    }

    /// Matches `particle` greedily from `pos`; the new position, or `None`
    /// when its minimum occurrence is not met.
    fn match_particle<'s>(
        &'s self,
        particle: &'s Particle,
        children: &[Node],
        pos: usize,
        matched: &mut Vec<Option<&'s ElementDecl>>,
    ) -> Option<usize> {
        let (min, max) = match particle {
            Particle::Element(_, min, max)
            | Particle::Sequence(_, min, max)
            | Particle::Choice(_, min, max) => (*min, *max),
        };
        let mut pos = pos;
        let mut count = 0u32;
        while count < max {
            let next = match particle {
                Particle::Element(decl, ..) => {
                    let decl = self.resolve(decl);
                    match children.get(pos) {
                        Some(child) if child.tag_name().name() == decl.name => {
                            matched[pos] = Some(decl);
                            Some(pos + 1)
                        }
                        _ => None,
                    }
                }
                Particle::Sequence(items, ..) => {
                    let mut at = Some(pos);
                    for item in items {
                        at = at.and_then(|p| self.match_particle(item, children, p, matched));
                    }
                    at
                }
                Particle::Choice(items, ..) => items
                    .iter()
                    .filter_map(|item| self.match_particle(item, children, pos, matched))
                    .find(|&p| p > pos),
            };
            match next {
                Some(next) if next > pos => {
                    pos = next;
                    count += 1;
                }
                // A sequence of optional items matched nothing: count it
                // once so `minOccurs` of an all-optional group is met.
                Some(_) => {
                    count = count.max(min);
                    break;
                }
                None => break,
            }
        }
        (count >= min).then_some(pos)
    }
}

#[derive(Default)]
struct State {
    errors: Vec<String>,
    ids: HashSet<String>,
    idrefs: Vec<(String, String)>,
}

fn names(nodes: &[Node]) -> String {
    nodes
        .iter()
        .take(4)
        .map(|n| format!("<{}>", n.tag_name().name()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn check_simple(ty: &Simple, value: &str) -> Result<(), String> {
    match ty {
        Simple::Enum(values) => {
            if values.iter().any(|v| v == value) {
                Ok(())
            } else {
                Err(format!("`{value}` is not one of {values:?}"))
            }
        }
        Simple::List(item) => value
            .split_whitespace()
            .try_for_each(|part| check_simple(item, part)),
        Simple::Builtin(name) => {
            let ok = match name.as_str() {
                "double" => {
                    matches!(value, "INF" | "-INF" | "NaN") || value.trim().parse::<f64>().is_ok()
                }
                "int" => value.trim().parse::<i32>().is_ok(),
                "boolean" => matches!(value.trim(), "true" | "false" | "0" | "1"),
                "ID" | "IDREF" => {
                    let mut chars = value.chars();
                    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                        && chars.all(|c| c.is_alphanumeric() || "_-.".contains(c))
                }
                _ => true,
            };
            if ok {
                Ok(())
            } else {
                Err(format!("`{value}` is not a valid xs:{name}"))
            }
        }
    }
}
