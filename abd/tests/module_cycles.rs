// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

mod source;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use proc_macro2::{TokenStream, TokenTree};
use source::{Module, ROOTS, Target, item_attrs, walk_files, workspace};
use syn::{
    Item, ItemUse, UseTree,
    ext::IdentExt as _,
    spanned::Spanned as _,
    visit::{self, Visit},
};

#[test]
fn production_rust_modules_are_acyclic() {
    let root = workspace();
    let targets = [
        target(std::env::consts::OS, std::env::consts::ARCH),
        target("android", "aarch64"),
        target("android", "x86_64"),
    ];
    let mut report = String::new();
    for source in ROOTS {
        for target in targets {
            let graph = scan(&root.join(source), target).graph();
            if let Some(cycle) = shortest_cycle(&graph) {
                writeln!(report, "{source} ({target}):").unwrap();
                report_cycle(&mut report, &graph, &cycle, root);
            }
        }
    }
    assert!(
        report.is_empty(),
        "Rust module dependency cycles:\n{report}"
    );
}

type Graph = BTreeMap<Module, BTreeMap<Module, Site>>;

#[derive(Clone)]
struct Site {
    file: PathBuf,
    line: usize,
}

struct Reference {
    from: Module,
    path: Vec<String>,
    site: Site,
}

#[derive(Default)]
struct Project {
    modules: BTreeSet<Module>,
    aliases: BTreeMap<Vec<String>, Vec<String>>,
    references: Vec<Reference>,
}

fn target(os: &'static str, arch: &'static str) -> Target {
    Target {
        os: Some(os),
        arch: Some(arch),
    }
}

fn scan(path: &Path, target: Target) -> Project {
    let mut project = Project::default();
    walk_files(path, target, &mut |module, file, _, syntax| {
        project.collect(module, file, &syntax.items, target);
    });
    project
}

impl Project {
    fn collect(&mut self, module: &[String], file: &Path, items: &[Item], target: Target) {
        self.modules.insert(module.to_vec());
        for item in items {
            if !target.includes(item_attrs(item)) {
                continue;
            }
            if let Item::Mod(child) = item {
                if let Some((_, items)) = &child.content {
                    let mut child_path = module.to_vec();
                    child_path.push(child.ident.unraw().to_string());
                    self.collect(&child_path, file, items, target);
                }
                continue;
            }
            let mut collector = References {
                target,
                module,
                file,
                references: &mut self.references,
            };
            if let Item::Use(import) = item {
                collect_use(import, |path, alias, line| {
                    if let Some(alias) = alias {
                        let mut key = module.to_vec();
                        key.push(alias);
                        self.aliases.insert(key, path.clone());
                    }
                    collector.record(path, line);
                });
            } else {
                collector.visit_item(item);
            }
        }
    }

    fn graph(&self) -> Graph {
        let mut graph = Graph::new();
        for reference in &self.references {
            let Some(path) = absolute_path(&reference.from, &reference.path) else {
                continue;
            };
            for to in self.resolve(path) {
                add_sibling_edge(&mut graph, &reference.from, to, &reference.site);
            }
        }
        graph
    }

    fn resolve(&self, mut path: Vec<String>) -> Vec<&Module> {
        let mut modules = Vec::new();
        let mut followed = BTreeSet::new();
        loop {
            let owner = (0..=path.len())
                .rev()
                .find_map(|end| self.modules.get(&path[..end]))
                .unwrap();
            modules.push(owner);

            // A qualified path can name an alias declared by an ancestor or
            // a facade. Credit the module that provides the imported name too.
            let alias_end = owner.len() + 1;
            let Some(key) = path.get(..alias_end) else {
                break;
            };
            let Some(alias) = self.aliases.get(key) else {
                break;
            };
            if !followed.insert(key.to_vec()) {
                break;
            }
            let Some(mut expanded) = absolute_path(owner, alias) else {
                break;
            };
            expanded.extend_from_slice(&path[alias_end..]);
            path = expanded;
        }
        modules
    }
}

struct References<'a> {
    target: Target,
    module: &'a [String],
    file: &'a Path,
    references: &'a mut Vec<Reference>,
}

impl References<'_> {
    fn record(&mut self, path: Vec<String>, line: usize) {
        self.references.push(Reference {
            from: self.module.to_vec(),
            path,
            site: Site {
                file: self.file.to_owned(),
                line,
            },
        });
    }
}

impl<'ast> Visit<'ast> for References<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if self.target.includes(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {
        // Module bodies have their own owner and are visited separately.
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        collect_use(item, |path, _, line| self.record(path, line));
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if self.target.includes(&item.attrs) {
            visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        if self.target.includes(&item.attrs) {
            visit::visit_trait_item_fn(self, item);
        }
    }

    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {
        // Attribute paths name attributes and cfg predicates, not module items.
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.ident.is_none() {
            visit::visit_item_macro(self, item);
        }
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        visit::visit_macro(self, mac);
        macro_paths(mac.tokens.clone(), &mut |path, line| {
            self.record(path, line)
        });
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.leading_colon.is_none()
            && path
                .segments
                .first()
                .is_some_and(|part| qualified_root(&part.ident))
        {
            self.record(
                path.segments
                    .iter()
                    .map(|part| part.ident.unraw().to_string())
                    .collect(),
                path.span().start().line,
            );
        }
        visit::visit_path(self, path);
    }
}

// Syn leaves macro inputs as tokens. Follow explicit paths in invocations
// without treating quoted strings, comments or macro definitions as code.
fn macro_paths(tokens: TokenStream, record: &mut impl FnMut(Vec<String>, usize)) {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Group(group) => macro_paths(group.stream(), record),
            TokenTree::Ident(ident) if qualified_root(&ident) => {
                let line = ident.span().start().line;
                let mut path = vec![ident.to_string()];
                while tokens.next_if(colon).is_some() && tokens.next_if(colon).is_some() {
                    let Some(TokenTree::Ident(part)) =
                        tokens.next_if(|token| matches!(token, TokenTree::Ident(_)))
                    else {
                        break;
                    };
                    path.push(part.unraw().to_string());
                }
                if path.len() > 1 {
                    record(path, line);
                }
            }
            _ => {}
        }
    }
}

fn colon(token: &TokenTree) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == ':')
}

fn qualified_root(ident: &syn::Ident) -> bool {
    ident == "crate" || ident == "super" || ident == "self"
}

fn collect_use(item: &ItemUse, mut record: impl FnMut(Vec<String>, Option<String>, usize)) {
    if item.leading_colon.is_none() {
        use_tree(&item.tree, &mut Vec::new(), &mut record);
    }
}

fn use_tree(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    record: &mut impl FnMut(Vec<String>, Option<String>, usize),
) {
    let (leaf, alias) = match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.unraw().to_string());
            use_tree(&path.tree, prefix, record);
            prefix.pop();
            return;
        }
        UseTree::Group(group) => {
            for tree in &group.items {
                use_tree(tree, prefix, record);
            }
            return;
        }
        UseTree::Name(name) => {
            let leaf = name.ident.unraw().to_string();
            let alias = if leaf == "self" {
                prefix.last().cloned()
            } else {
                Some(leaf.clone())
            };
            (Some(leaf), alias)
        }
        UseTree::Rename(rename) => (
            Some(rename.ident.unraw().to_string()),
            Some(rename.rename.unraw().to_string()),
        ),
        UseTree::Glob(_) => (None, None),
    };
    let mut path = prefix.clone();
    path.extend(leaf.filter(|leaf| leaf != "self"));
    record(
        path,
        alias.filter(|alias| alias != "_"),
        tree.span().start().line,
    );
}

fn absolute_path(from: &[String], parts: &[String]) -> Option<Vec<String>> {
    let mut path = from.to_vec();
    let mut parts = parts.iter().peekable();
    match parts.peek().map(|part| part.as_str()) {
        Some("crate") => {
            path.clear();
            parts.next();
        }
        Some("self") => {
            parts.next();
        }
        _ => {}
    }
    while parts.peek().is_some_and(|part| *part == "super") {
        path.pop()?;
        parts.next();
    }
    path.extend(parts.cloned());
    Some(path)
}

fn add_sibling_edge(graph: &mut Graph, from: &[String], to: &[String], site: &Site) {
    // A dependency between descendants also couples the enclosing siblings.
    if let Some(depth) = from.iter().zip(to).position(|(a, b)| a != b) {
        graph
            .entry(from[..=depth].to_vec())
            .or_default()
            .entry(to[..=depth].to_vec())
            .or_insert_with(|| site.clone());
    }
}

fn shortest_cycle(graph: &Graph) -> Option<Vec<&Module>> {
    graph
        .keys()
        .filter_map(|start| cycle_from(graph, start))
        .min_by_key(Vec::len)
}

fn cycle_from<'a>(graph: &'a Graph, start: &'a Module) -> Option<Vec<&'a Module>> {
    let mut queue = VecDeque::from([start]);
    let mut previous = BTreeMap::from([(start, start)]);
    while let Some(from) = queue.pop_front() {
        for to in graph.get(from).into_iter().flat_map(|edges| edges.keys()) {
            if to == start {
                let mut path = vec![start, from];
                while *path.last().unwrap() != start {
                    path.push(previous[path.last().unwrap()]);
                }
                path.reverse();
                return Some(path);
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = previous.entry(to) {
                entry.insert(from);
                queue.push_back(to);
            }
        }
    }
    None
}

fn report_cycle(out: &mut String, graph: &Graph, cycle: &[&Module], root: &Path) {
    for edge in cycle.windows(2) {
        let [from, to] = edge else { unreachable!() };
        let site = &graph[*from][*to];
        let file = site.file.strip_prefix(root).unwrap_or(&site.file);
        writeln!(
            out,
            "  {} -> {} at {}:{}",
            from.join("::"),
            to.join("::"),
            file.display(),
            site.line
        )
        .unwrap();
    }
}

#[test]
fn follows_reexports_and_scoped_imports_to_report_the_shortest_cycle() {
    let dir = fixture(&[
        (
            "lib.rs",
            "mod a;\nmod b;\nmod c;\npub use b::thing as exported;\n",
        ),
        (
            "a.rs",
            "#[path = \"nested.rs\"] mod nested;\npub fn item() {}\n",
        ),
        ("nested.rs", "pub fn run() { crate::exported(); }\n"),
        (
            "b/mod.rs",
            "pub fn thing() {\n    use super::{a as first, c};\n    first::item();\n}\n",
        ),
        ("c.rs", "pub fn longer() { super::a::item(); }\n"),
    ]);
    let graph = scan(&dir.path().join("lib.rs"), Target::default()).graph();
    let mut report = String::new();
    report_cycle(
        &mut report,
        &graph,
        &shortest_cycle(&graph).unwrap(),
        dir.path(),
    );
    assert_eq!(report, "  a -> b at nested.rs:1\n  b -> a at b/mod.rs:2\n");
}

#[test]
fn projects_qualified_paths_from_nested_modules_to_sibling_edges() {
    let dir = fixture(&[
        (
            "lib.rs",
            "mod parent {\n    mod a {\n        pub(super) mod inner;\n    }\n    mod b {\n        use super::a::inner;\n        pub fn run() {}\n    }\n}\n",
        ),
        (
            "parent/a/inner.rs",
            "pub use super::super::b::{self as sibling, run};\n",
        ),
    ]);
    let graph = scan(&dir.path().join("lib.rs"), Target::default()).graph();
    let mut report = String::new();
    report_cycle(
        &mut report,
        &graph,
        &shortest_cycle(&graph).unwrap(),
        dir.path(),
    );
    assert_eq!(
        report,
        "  parent::a -> parent::b at parent/a/inner.rs:1\n  parent::b -> parent::a at lib.rs:6\n",
    );
}

#[test]
fn excludes_test_modules_and_keeps_target_specific_dependencies_separate() {
    let dir = fixture(&[(
        "lib.rs",
        r#"
#[cfg(all(test, target_os = "linux"))]
mod test_fixture;
mod a {
    #[cfg(not(target_os = "android"))]
    use super::b::run as other;
    #[test]
    fn test_only() { crate::b::run(); }
    const EXAMPLE: &str = "use super::b::run;";
    pub fn run() {}
}
mod b {
    #[cfg(any(test, all(target_os = "android", target_arch = "aarch64")))]
    use super::a::run as other;
    #[cfg(test)]
    mod internal { use crate::a::run; }
    pub fn run() {}
}
"#,
    )]);
    let entry = dir.path().join("lib.rs");
    assert!(shortest_cycle(&scan(&entry, Target::default()).graph()).is_some());
    for target in [
        target("linux", "x86_64"),
        target("android", "aarch64"),
        target("android", "x86_64"),
    ] {
        assert!(shortest_cycle(&scan(&entry, target).graph()).is_none());
    }
}

#[test]
fn follows_qualified_paths_in_macro_arguments_without_expanding_macros() {
    let dir = fixture(&[(
        "lib.rs",
        r#"
mod a {
    pub fn render() { println!("{}", crate::b::name()); }
}
mod b {
    pub fn name() -> &'static str { "crate::a::render()" }
    macro_rules! skipped { () => { crate::a::render() } }
}
mod c {
    use super::a::render;
    pub fn report() { let _ = format!("{:?}", super::a::render()); }
}
"#,
    )]);
    let entry = dir.path().join("lib.rs");
    let project = scan(&entry, Target::default());
    let graph = project.graph();
    assert!(graph[&vec!["a".to_owned()]].contains_key(&vec!["b".to_owned()]));
    assert!(shortest_cycle(&graph).is_none());
}

fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    dir
}
