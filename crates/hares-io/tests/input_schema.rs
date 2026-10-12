//! The machine-readable input schema's HPXML-element surface: the parser's
//! rejections go through the declared `HPXML_REJECTED_ELEMENTS` list, so
//! `base.xml` with a listed element added under its context fails the parse
//! with the entry's reason, and the list is exactly what the schema
//! publishes.

use std::path::PathBuf;

use hares_io::hpxml::elements::HPXML_REJECTED_ELEMENTS;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn base_xml() -> String {
    let path = project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join("base.xml");
    std::fs::read_to_string(&path).expect("base.xml fixture reads")
}

/// Insert `<element/>` under the context path's LAST element (the parent
/// the parser reads the rejected element under), as the acceptance adds
/// them: `base.xml` with that element added under its context fails the
/// parse.
fn with_element_under(xml: &str, context: &str, name: &str) -> String {
    let segments: Vec<&str> = context.split('/').collect();
    // Find the LAST occurrence of the context's final element's opening tag
    // and inject the rejected element as its first child. The fixtures are
    // indented; the injection matches the fixture's own four-space step.
    let parent = *segments.last().expect("a context has a final segment");
    let open = format!("<{parent}");
    let insert = format!("<{name}/>");
    let mut out_end = 0;
    loop {
        let Some(pos) = xml[out_end..].find(&open) else {
            panic!("context element <{parent}> not found in the fixture");
        };
        let abs = out_end + pos;
        // Only a self-contained open tag: the char after the name must not
        // extend the element's name.
        let after = xml
            .get(abs + open.len()..)
            .and_then(|tail| tail.chars().next());
        let is_name_end = !matches!(after, Some(c) if c.is_alphanumeric() || c == ':' || c == '_');
        if !is_name_end {
            out_end = abs + open.len();
            continue;
        }
        // Skip the element's own children: inject just before its close.
        let close = format!("</{parent}>");
        let close_pos = xml[abs..].find(&close).expect("a closing tag") + abs;
        let mut out = String::with_capacity(xml.len() + insert.len() + 2);
        out.push_str(&xml[..close_pos]);
        out.push_str(&insert);
        out.push_str(&xml[close_pos..]);
        return out;
    }
}

#[test]
fn rejected_elements_fail_the_parse_as_listed() {
    assert!(
        !HPXML_REJECTED_ELEMENTS.is_empty(),
        "the rejected-elements list is not empty on this base: it carries \
         every element the parser refuses"
    );
    for entry in HPXML_REJECTED_ELEMENTS {
        let xml = with_element_under(&base_xml(), entry.context, entry.name);
        let err = hares_io::hpxml::parse_hpxml_str(&xml)
            .expect_err("a listed rejected element fails the parse");
        let message = err.to_string();
        assert!(
            message.contains(entry.name) && message.contains(entry.reason),
            "the parse error names the element and the entry's reason: \
             element {}, reason {:?}, error: {message}",
            entry.name,
            entry.reason,
        );
    }
}
