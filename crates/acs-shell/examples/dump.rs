//! Debug helper: print the tree-sitter-bash tree of a command read from a file.
#![allow(missing_docs, clippy::unwrap_used)]
use tree_sitter::{Node, Parser};

fn show(n: Node, src: &str, depth: usize, field: Option<&str>) {
    let text: String = src[n.byte_range()].chars().take(30).collect();
    println!(
        "{}{}{}{} {:?}",
        "  ".repeat(depth),
        field.map(|f| format!("{f}: ")).unwrap_or_default(),
        n.kind(),
        if n.is_named() { "" } else { " (anon)" },
        text
    );
    let mut c = n.walk();
    for (i, ch) in n.children(&mut c).enumerate() {
        let f = n.field_name_for_child(i as u32);
        if ch.is_named() || ch.kind().chars().all(|x| !x.is_alphanumeric()) {
            show(ch, src, depth + 1, f);
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(path).unwrap();
    let mut p = Parser::new();
    p.set_language(&tree_sitter_bash::LANGUAGE.into()).unwrap();
    let t = p.parse(&src, None).unwrap();
    show(t.root_node(), &src, 0, None);
}
