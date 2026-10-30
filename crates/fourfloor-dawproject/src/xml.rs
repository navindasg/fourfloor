//! A small XML writer, and helpers for reading attributes from the
//! `roxmltree` tree. Writing by hand keeps the output byte-stable and the
//! attribute order under our control.

use std::fmt::Write as _;

/// An element under construction: a name and its attributes in order.
pub(crate) struct El {
    name: &'static str,
    attrs: String,
}

impl El {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            attrs: String::new(),
        }
    }

    /// Adds an attribute. The value is escaped.
    pub(crate) fn attr(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        let raw = value.to_string();
        // Writing to a String cannot fail.
        let _ = write!(self.attrs, " {key}=\"");
        escape_into(&mut self.attrs, &raw);
        self.attrs.push('"');
        self
    }

    /// Adds an attribute when there is a value.
    pub(crate) fn opt(self, key: &str, value: Option<impl std::fmt::Display>) -> Self {
        match value {
            Some(value) => self.attr(key, value),
            None => self,
        }
    }
}

fn escape_into(out: &mut String, raw: &str) {
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            // Other control characters cannot appear in XML 1.0 at all.
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
}

/// Writes an indented XML document.
pub(crate) struct XmlWriter {
    out: String,
    open: Vec<&'static str>,
}

impl XmlWriter {
    pub(crate) fn new() -> Self {
        Self {
            out: "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n".to_owned(),
            open: Vec::new(),
        }
    }

    fn indent(&mut self) {
        for _ in 0..self.open.len() {
            self.out.push_str("  ");
        }
    }

    /// Writes `<name attrs/>`.
    pub(crate) fn leaf(&mut self, el: El) {
        self.indent();
        let _ = writeln!(self.out, "<{}{}/>", el.name, el.attrs);
    }

    /// Writes `<name attrs>` and makes it the current element.
    pub(crate) fn open(&mut self, el: El) {
        self.indent();
        let _ = writeln!(self.out, "<{}{}>", el.name, el.attrs);
        self.open.push(el.name);
    }

    /// Closes the current element.
    pub(crate) fn close(&mut self) {
        if let Some(name) = self.open.pop() {
            self.indent();
            let _ = writeln!(self.out, "</{name}>");
        }
    }

    /// Writes `<name>text</name>`.
    pub(crate) fn text(&mut self, name: &'static str, text: &str) {
        self.indent();
        let _ = write!(self.out, "<{name}>");
        escape_into(&mut self.out, text);
        let _ = writeln!(self.out, "</{name}>");
    }

    /// Closes everything still open and returns the document.
    pub(crate) fn finish(mut self) -> String {
        while !self.open.is_empty() {
            self.close();
        }
        self.out
    }
}

pub(crate) type Node<'a, 'input> = roxmltree::Node<'a, 'input>;

/// The element children of a node.
pub(crate) fn elements<'a, 'input>(
    node: Node<'a, 'input>,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

/// The first child element with this name.
pub(crate) fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    elements(node).find(|child| child.tag_name().name() == name)
}

/// The element's tag name.
pub(crate) fn tag<'a>(node: Node<'a, '_>) -> &'a str {
    node.tag_name().name()
}

/// A finite number attribute.
pub(crate) fn number(node: Node<'_, '_>, name: &str) -> Option<f64> {
    node.attribute(name)
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

/// An integer attribute.
pub(crate) fn integer(node: Node<'_, '_>, name: &str) -> Option<i64> {
    node.attribute(name)
        .and_then(|text| text.trim().parse::<i64>().ok())
}

/// An `xs:boolean` attribute (`true`, `false`, `1` or `0`).
pub(crate) fn boolean(node: Node<'_, '_>, name: &str) -> Option<bool> {
    match node.attribute(name).map(str::trim) {
        Some("true" | "1") => Some(true),
        Some("false" | "0") => Some(false),
        _ => None,
    }
}

/// A path such as `Project/Structure/Track[Bass]` for report locations.
pub(crate) fn path(node: Node<'_, '_>) -> String {
    let mut parts: Vec<String> = node
        .ancestors()
        .filter(Node::is_element)
        .map(
            |n| match n.attribute("name").or_else(|| n.attribute("id")) {
                Some(label) => format!("{}[{label}]", tag(n)),
                None => tag(n).to_owned(),
            },
        )
        .collect();
    parts.reverse();
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_escapes_and_nests() {
        let mut w = XmlWriter::new();
        w.open(El::new("A").attr("name", "R&B <\"1\">\n"));
        w.leaf(
            El::new("B")
                .attr("x", 1.5)
                .opt("y", None::<u8>)
                .opt("z", Some(2)),
        );
        w.text("T", "a < b");
        let text = w.finish();
        let doc = roxmltree::Document::parse(&text).unwrap();
        let a = doc.root_element();
        assert_eq!(a.attribute("name"), Some("R&B <\"1\">\n"));
        let b = child(a, "B").unwrap();
        assert_eq!(number(b, "x"), Some(1.5));
        assert_eq!(b.attribute("y"), None);
        assert_eq!(integer(b, "z"), Some(2));
        assert_eq!(child(a, "T").unwrap().text(), Some("a < b"));
        assert_eq!(path(b), "A[R&B <\"1\">\n]/B");
    }

    #[test]
    fn attribute_readers_reject_junk() {
        let doc = roxmltree::Document::parse(r#"<A n="NaN" b="yes" t="1" i="2.5"/>"#).unwrap();
        let a = doc.root_element();
        assert_eq!(number(a, "n"), None);
        assert_eq!(boolean(a, "b"), None);
        assert_eq!(boolean(a, "t"), Some(true));
        assert_eq!(integer(a, "i"), None);
    }
}
