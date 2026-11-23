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

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, File, Item, ItemMod, Lit, Meta, ext::IdentExt as _, punctuated::Punctuated};

pub const ROOTS: &[&str] = &[
    "abd/src/main.rs",
    "abd-engine/src/lib.rs",
    "abd-vpn/src/lib.rs",
    "abd-vpn/examples/check_http3.rs",
];

pub fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

pub type Module = Vec<String>;
type FileVisitor<'a> = dyn FnMut(&[String], &Path, &str, &File) + 'a;

pub fn walk_files(path: &Path, target: Target, visit: &mut FileVisitor<'_>) {
    walk_file(path, path.parent().unwrap(), &mut Vec::new(), target, visit);
}

fn walk_file(
    path: &Path,
    module_dir: &Path,
    module: &mut Module,
    target: Target,
    visit: &mut FileVisitor<'_>,
) {
    let source = fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let syntax = syn::parse_file(&source).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    if target.includes(&syntax.attrs) {
        visit(module, path, &source, &syntax);
        walk_children(
            &syntax.items,
            module_dir,
            path.parent().unwrap(),
            module,
            target,
            visit,
        );
    }
}

fn walk_children(
    items: &[Item],
    module_dir: &Path,
    path_attr_dir: &Path,
    parent: &mut Module,
    target: Target,
    visit: &mut FileVisitor<'_>,
) {
    for item in items {
        let Item::Mod(module) = item else { continue };
        if !target.includes(&module.attrs) {
            continue;
        }
        let name = module.ident.unraw().to_string();
        let child_dir = module_dir.join(&name);
        let explicit_path = path_attribute(module).map(|path| path_attr_dir.join(path));
        let flat = module_dir.join(format!("{name}.rs"));
        parent.push(name);
        if let Some((_, items)) = &module.content {
            let dir = explicit_path.as_deref().unwrap_or(&child_dir);
            walk_children(items, dir, dir, parent, target, visit);
        } else if let Some(path) = explicit_path {
            walk_file(&path, path.parent().unwrap(), parent, target, visit);
        } else {
            let path = if flat.is_file() {
                flat
            } else {
                child_dir.join("mod.rs")
            };
            walk_file(&path, &child_dir, parent, target, visit);
        }
        parent.pop();
    }
}

fn path_attribute(module: &ItemMod) -> Option<PathBuf> {
    module
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("path"))
        .map(|attr| {
            let value = attr.meta.require_name_value().unwrap();
            let syn::Expr::Lit(expr) = &value.value else {
                panic!("expected a path literal")
            };
            let Lit::Str(value) = &expr.lit else {
                panic!("expected a path string")
            };
            PathBuf::from(value.value())
        })
}

#[derive(Clone, Copy, Default)]
pub struct Target {
    pub os: Option<&'static str>,
    pub arch: Option<&'static str>,
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            self.arch.unwrap_or("any"),
            self.os.unwrap_or("any")
        )
    }
}

impl Target {
    pub fn includes(self, attrs: &[Attribute]) -> bool {
        attrs.iter().all(|attr| {
            attr.path().segments.last().unwrap().ident != "test" && self.keeps(&attr.meta)
        })
    }

    fn keeps(self, meta: &Meta) -> bool {
        let Meta::List(list) = meta else { return true };
        if list.path.is_ident("cfg") {
            return self.cfg_value(&list.parse_args().unwrap()) != Some(false);
        }
        if list.path.is_ident("cfg_attr") {
            let args = cfg_args(list);
            let mut args = args.iter();
            let condition = args.next().unwrap();
            return self.cfg_value(condition) != Some(true) || args.all(|meta| self.keeps(meta));
        }
        true
    }

    // Unknown predicates leave both branches eligible. The size check leaves
    // the target unspecified so it includes all platform-specific code.
    fn cfg_value(self, meta: &Meta) -> Option<bool> {
        if meta.path().is_ident("test") {
            return Some(false);
        }
        match meta {
            Meta::NameValue(value) if value.path.is_ident("target_os") => {
                self.os.map(|os| os == cfg_string(&value.value))
            }
            Meta::NameValue(value) if value.path.is_ident("target_arch") => {
                self.arch.map(|arch| arch == cfg_string(&value.value))
            }
            Meta::List(list) if list.path.is_ident("not") => self
                .cfg_value(&list.parse_args().unwrap())
                .map(|value| !value),
            Meta::List(list) if list.path.is_ident("any") || list.path.is_ident("all") => {
                self.cfg_group(list)
            }
            _ => None,
        }
    }

    fn cfg_group(self, list: &syn::MetaList) -> Option<bool> {
        let triggering = list.path.is_ident("any");
        let mut known = true;
        for arg in cfg_args(list) {
            match self.cfg_value(&arg) {
                Some(value) if value == triggering => return Some(triggering),
                Some(_) => {}
                None => known = false,
            }
        }
        known.then_some(!triggering)
    }
}

fn cfg_args(list: &syn::MetaList) -> Punctuated<Meta, syn::Token![,]> {
    list.parse_args_with(Punctuated::parse_terminated).unwrap()
}

fn cfg_string(value: &syn::Expr) -> String {
    let syn::Expr::Lit(expr) = value else {
        panic!("expected a cfg literal")
    };
    let Lit::Str(value) = &expr.lit else {
        panic!("expected a cfg string")
    };
    value.value()
}

pub fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}
