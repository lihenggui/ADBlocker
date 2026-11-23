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

use std::path::Path;

use source::{ROOTS, Target, item_attrs, walk_files, workspace};
use syn::{
    File, Item,
    spanned::Spanned,
    visit::{self, Visit},
};

const LIMIT: usize = 800;

#[test]
fn production_rust_files_fit_the_line_limit() {
    let root = workspace();
    let mut oversized = Vec::new();
    for source in ROOTS {
        walk_files(
            &root.join(source),
            Target::default(),
            &mut |_, path, contents, syntax| {
                if let Some(issue) = size_issue(path.strip_prefix(root).unwrap(), contents, syntax)
                {
                    oversized.push(issue);
                }
            },
        );
    }
    assert!(oversized.is_empty(), "{}", oversized.join("\n"));
}

fn size_issue(path: &Path, source: &str, syntax: &File) -> Option<String> {
    let mut lines = ProductionLines(vec![true; source.lines().count()]);
    lines.visit_file(syntax);
    let count = lines.0.into_iter().filter(|&production| production).count();
    (count > LIMIT).then(|| {
        format!(
            "{} has {count} production lines (limit {LIMIT})",
            path.display()
        )
    })
}

struct ProductionLines(Vec<bool>);

impl<'ast> Visit<'ast> for ProductionLines {
    fn visit_item(&mut self, item: &'ast Item) {
        if !Target::default().includes(item_attrs(item)) {
            let span = item.span();
            self.0[span.start().line - 1..span.end().line].fill(false);
        } else {
            visit::visit_item(self, item);
        }
    }
}

#[test]
fn counts_production_after_test_items_and_accepts_the_exact_limit() {
    let mut source = "#[cfg(test)]\nmod tests {\n    fn helper() {}\n}\n\n".to_owned();
    source.push_str(&"// production comment\n".repeat(LIMIT - 1));
    let path = Path::new("sample.rs");
    assert_eq!(
        size_issue(path, &source, &syn::parse_file(&source).unwrap()),
        None
    );
    source.push_str("fn production() {}\n");
    assert_eq!(
        size_issue(path, &source, &syn::parse_file(&source).unwrap()).as_deref(),
        Some("sample.rs has 801 production lines (limit 800)"),
    );
}
