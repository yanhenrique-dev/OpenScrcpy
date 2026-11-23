//! Generated-catalog parity gate.
//!
//! `locales/ui.yml` is produced by `scripts/export-i18n.mjs`, which already
//! fails on missing keys. This test is the gate that runs with the ordinary
//! Rust suite and proves the shipped catalog still carries every locale: same
//! key set, no empty value, and the same `%{placeholder}` set per key.

use std::collections::{BTreeMap, BTreeSet};

use openscrcpy_gpui::SUPPORTED_LOCALES;

#[derive(Debug)]
enum Node {
    Branch(BTreeMap<String, Node>),
    Text(String),
}

/// Minimal parser for the generated subset: two-space indentation, `key:` maps
/// and `key: "value"` leaves. Full YAML validity is enforced when
/// `rust_i18n::i18n!` embeds the file at compile time.
fn parse(source: &str) -> Node {
    let mut root = BTreeMap::new();
    let mut stack: Vec<(usize, String)> = Vec::new();

    for line in source.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.trim() == "_version: 2" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        while stack.last().is_some_and(|(depth, _)| *depth >= indent) {
            stack.pop();
        }
        let Some((key, rest)) = line[indent..].split_once(':') else {
            panic!("generated catalog line must contain a key: {line}");
        };
        let key = key.trim().to_string();
        let rest = rest.trim();

        // Walk from the root to the parent branch of this line.
        let mut node = &mut root;
        for (_, part) in &stack {
            node = match node
                .entry(part.clone())
                .or_insert_with(|| Node::Branch(BTreeMap::new()))
            {
                Node::Branch(branch) => branch,
                Node::Text(_) => panic!("{part} is used as both a key and a value"),
            };
        }

        if rest.is_empty() {
            stack.push((indent, key.clone()));
            node.insert(key, Node::Branch(BTreeMap::new()));
        } else {
            let value = rest
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .expect("generated values are double-quoted")
                .replace("\\\\", "\\")
                .replace("\\\"", "\"")
                .replace("\\n", "\n");
            node.insert(key, Node::Text(value));
        }
    }

    Node::Branch(root)
}

fn placeholders(value: &str) -> BTreeSet<&str> {
    let mut found = BTreeSet::new();
    let mut rest = value;
    while let Some(start) = rest.find("%{") {
        let tail = &rest[start + 2..];
        let Some(end) = tail.find('}') else { break };
        found.insert(&tail[..end]);
        rest = &tail[end..];
    }
    found
}

/// Collects every leaf whose children are locale codes: `key -> locale -> text`.
fn collect_leaves(
    node: &Node,
    path: &mut String,
    out: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    let Node::Branch(branch) = node else {
        return;
    };
    let is_locale_map = !branch.is_empty()
        && branch.values().all(|child| matches!(child, Node::Text(_)))
        && branch
            .keys()
            .all(|key| SUPPORTED_LOCALES.contains(&key.as_str()));
    if is_locale_map {
        let mut map = BTreeMap::new();
        for (code, child) in branch {
            if let Node::Text(text) = child {
                map.insert(code.clone(), text.clone());
            }
        }
        out.insert(path.clone(), map);
        return;
    }
    for (key, child) in branch {
        let previous = path.len();
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
        collect_leaves(child, path, out);
        path.truncate(previous);
    }
}

fn catalog() -> BTreeMap<String, BTreeMap<String, String>> {
    let root = parse(include_str!("../locales/ui.yml"));
    let mut leaves = BTreeMap::new();
    collect_leaves(&root, &mut String::new(), &mut leaves);
    leaves
}

#[test]
fn catalog_has_every_supported_locale_for_every_key() {
    let leaves = catalog();
    assert_eq!(leaves.len(), 393, "unexpected number of keys");
    for (key, translations) in &leaves {
        let present: BTreeSet<_> = translations.keys().map(String::as_str).collect();
        let expected: BTreeSet<_> = SUPPORTED_LOCALES.into_iter().collect();
        assert_eq!(&present, &expected, "{key} does not ship every locale");
        for (code, text) in translations {
            assert!(
                !text.trim().is_empty(),
                "{code} has an empty value for {key}"
            );
        }
    }
}

#[test]
fn locales_keep_the_same_placeholders() {
    let leaves = catalog();
    for (key, translations) in &leaves {
        let Some(reference) = translations.get("en") else {
            panic!("{key} is missing the en fallback");
        };
        let expected = placeholders(reference);
        for (code, text) in translations {
            assert_eq!(
                expected,
                placeholders(text),
                "{code} changed the placeholders of {key}"
            );
        }
    }
}
