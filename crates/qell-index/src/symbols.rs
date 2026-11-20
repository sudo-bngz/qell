//! Definitions found in a file, without a language server.
//!
//! Most languages use their grammar's tree-sitter `tags.scm` (functions,
//! types, methods, modules…). Terraform has none, so its blocks are read
//! from the HCL tree directly and named the way they are referenced:
//! `var.region`, `local.name`, `module.vpc`, `data.aws_iam_policy_document.p`,
//! `aws_s3_bucket.logs`.

use std::collections::HashMap;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

/// A definition in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// What references use: an identifier, or a Terraform address.
    pub name: String,
    /// "function", "class", "module", "variable", "resource"…
    pub kind: String,
    /// 0-based line and char columns of the name.
    pub line: usize,
    pub col: usize,
    pub end_col: usize,
}

/// Per-thread extraction state: tree-sitter configurations are compiled
/// once per language.
pub struct Extractor {
    tags: TagsContext,
    configs: HashMap<&'static str, TagsConfiguration>,
    hcl: tree_sitter::Parser,
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor {
    pub fn new() -> Self {
        let mut configs = HashMap::new();
        // TypeScript's queries only add TS constructs (interfaces, abstract
        // classes…) on top of JavaScript's, like its grammar's `inherits`.
        let ts_tags = format!("{}\n{}", tree_sitter_javascript::TAGS_QUERY, tree_sitter_typescript::TAGS_QUERY);
        let ts_locals = format!("{}\n{}", tree_sitter_javascript::LOCALS_QUERY, tree_sitter_typescript::LOCALS_QUERY);
        let langs: [(&'static str, tree_sitter::Language, &str, &str); 6] = [
            ("rust", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::TAGS_QUERY, ""),
            ("c", tree_sitter_c::LANGUAGE.into(), tree_sitter_c::TAGS_QUERY, ""),
            ("go", tree_sitter_go::LANGUAGE.into(), tree_sitter_go::TAGS_QUERY, ""),
            (
                "javascript",
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::TAGS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &ts_tags, &ts_locals),
            ("typescriptreact", tree_sitter_typescript::LANGUAGE_TSX.into(), &ts_tags, &ts_locals),
        ];
        for (id, lang, tags, locals) in langs {
            match TagsConfiguration::new(lang, tags, locals) {
                Ok(c) => {
                    configs.insert(id, c);
                }
                Err(e) => log::warn!("tags query for {id}: {e}"),
            }
        }
        let mut hcl = tree_sitter::Parser::new();
        let _ = hcl.set_language(&tree_sitter_hcl::LANGUAGE.into());
        Self { tags: TagsContext::new(), configs, hcl }
    }

    /// An extractor that finds nothing (for workers that only read text).
    pub fn empty() -> Self {
        Self { tags: TagsContext::new(), configs: HashMap::new(), hcl: tree_sitter::Parser::new() }
    }

    /// Whether definitions can be found for this language.
    pub fn supports(&self, lang: &str) -> bool {
        lang == "terraform" || self.configs.contains_key(lang)
    }

    pub fn extract(&mut self, lang: &str, text: &str) -> Vec<Symbol> {
        if lang == "terraform" {
            return self.hcl_symbols(text);
        }
        let Some(config) = self.configs.get(lang) else { return Vec::new() };
        let Ok((tags, _)) = self.tags.generate_tags(config, text.as_bytes(), None) else { return Vec::new() };
        let mut out = Vec::new();
        for tag in tags.flatten() {
            if !tag.is_definition {
                continue;
            }
            let Some(name) = text.get(tag.name_range.clone()) else { continue };
            let line_start = text[..tag.name_range.start].rfind('\n').map_or(0, |i| i + 1);
            let col = text[line_start..tag.name_range.start].chars().count();
            out.push(Symbol {
                name: name.to_string(),
                kind: config.syntax_type_name(tag.syntax_type_id).to_string(),
                line: tag.span.start.row,
                col,
                end_col: col + name.chars().count(),
            });
        }
        out
    }

    fn hcl_symbols(&mut self, text: &str) -> Vec<Symbol> {
        let Some(tree) = self.hcl.parse(text, None) else { return Vec::new() };
        let src = text.as_bytes();
        let node_text = |n: tree_sitter::Node| n.utf8_text(src).unwrap_or("").to_string();
        let symbol = |name: String, kind: &str, n: tree_sitter::Node| {
            let p = n.start_position();
            let line_start = text[..n.start_byte()].rfind('\n').map_or(0, |i| i + 1);
            let col = text[line_start..n.start_byte()].chars().count();
            let len = n.utf8_text(src).map(|t| t.chars().count()).unwrap_or(0);
            Symbol { name, kind: kind.to_string(), line: p.row, col, end_col: col + len }
        };
        let mut out = Vec::new();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let Some(body) = root.named_children(&mut cursor).find(|n| n.kind() == "body") else { return out };
        let mut cursor = body.walk();
        for block in body.named_children(&mut cursor).filter(|n| n.kind() == "block") {
            let mut c = block.walk();
            let children: Vec<_> = block.named_children(&mut c).collect();
            let Some(kind) = children.iter().find(|n| n.kind() == "identifier").map(|n| node_text(*n)) else { continue };
            // Label nodes and their text without quotes.
            let labels: Vec<(tree_sitter::Node, String)> = children
                .iter()
                .filter(|n| n.kind() == "string_lit")
                .map(|n| {
                    let mut c = n.walk();
                    let inner = n.named_children(&mut c).find(|i| i.kind() == "template_literal").unwrap_or(*n);
                    (inner, node_text(inner))
                })
                .collect();
            match (kind.as_str(), labels.as_slice()) {
                ("variable", [(n, l)]) => out.push(symbol(format!("var.{l}"), "variable", *n)),
                ("module", [(n, l)]) => out.push(symbol(format!("module.{l}"), "module", *n)),
                ("output", [(n, l)]) => out.push(symbol(format!("output.{l}"), "output", *n)),
                ("resource", [(_, t), (n, l)]) => out.push(symbol(format!("{t}.{l}"), "resource", *n)),
                ("data", [(_, t), (n, l)]) => out.push(symbol(format!("data.{t}.{l}"), "data", *n)),
                ("locals", []) => {
                    if let Some(b) = children.iter().find(|n| n.kind() == "body") {
                        let mut c = b.walk();
                        for attr in b.named_children(&mut c).filter(|n| n.kind() == "attribute") {
                            let mut c2 = attr.walk();
                            let id = attr.named_children(&mut c2).find(|n| n.kind() == "identifier");
                            if let Some(id) = id {
                                out.push(symbol(format!("local.{}", node_text(id)), "local", id));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Symbol names a Terraform expression under the cursor may refer to,
/// longest first: `aws_s3_bucket.logs.arn` → `aws_s3_bucket.logs.arn`,
/// `aws_s3_bucket.logs`; `module.vpc.vpc_id` → …, `module.vpc`.
pub fn terraform_candidates(expr: &str) -> Vec<String> {
    let parts: Vec<&str> = expr.split('.').filter(|p| !p.is_empty()).collect();
    (2..=parts.len()).rev().map(|n| parts[..n].join(".")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_definitions() {
        let src = "struct Point { x: i32 }\n\nfn make_point() -> Point { Point { x: 1 } }\n\nimpl Point {\n    fn norm(&self) -> i32 { self.x }\n}\n";
        let syms = Extractor::new().extract("rust", src);
        let find = |n: &str| syms.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("{n} in {syms:?}"));
        assert_eq!((find("Point").line, find("Point").col), (0, 7));
        assert_eq!(find("make_point").line, 2);
        assert_eq!(find("norm").line, 5);
        assert_eq!(find("norm").col, 7);
    }

    #[test]
    fn go_and_typescript_definitions() {
        let mut x = Extractor::new();
        let go = x.extract("go", "package main\n\nfunc Hello() string { return \"hi\" }\n\ntype Server struct{}\n");
        assert!(go.iter().any(|s| s.name == "Hello" && s.line == 2));
        assert!(go.iter().any(|s| s.name == "Server"));
        let ts =
            x.extract("typescript", "export function formatDate(d: Date) { return d }\nclass Store {}\ninterface Shape {}\n");
        assert!(ts.iter().any(|s| s.name == "formatDate"), "{ts:?}");
        assert!(ts.iter().any(|s| s.name == "Store" && s.line == 1));
        assert!(ts.iter().any(|s| s.name == "Shape"), "TypeScript-only construct");
    }

    #[test]
    fn terraform_blocks() {
        let src = r#"variable "region" {
  default = "eu-west-3"
}
locals {
  name = "qell"
  env  = "prod"
}
resource "aws_s3_bucket" "logs" {
  bucket = "${local.name}-${var.region}"
}
data "aws_iam_policy_document" "p" {}
module "vpc" {
  source = "./vpc"
}
output "bucket" { value = aws_s3_bucket.logs.id }
"#;
        let syms = Extractor::new().extract("terraform", src);
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "var.region",
                "local.name",
                "local.env",
                "aws_s3_bucket.logs",
                "data.aws_iam_policy_document.p",
                "module.vpc",
                "output.bucket"
            ]
        );
        let logs = &syms[3];
        assert_eq!((logs.line, logs.col, logs.end_col), (7, 26, 30)); // the "logs" label
        assert_eq!((syms[1].line, syms[1].col), (4, 2)); // local.name
    }

    #[test]
    fn terraform_reference_candidates() {
        assert_eq!(terraform_candidates("aws_s3_bucket.logs.arn"), vec!["aws_s3_bucket.logs.arn", "aws_s3_bucket.logs"]);
        assert_eq!(terraform_candidates("var.region"), vec!["var.region"]);
        assert!(terraform_candidates("region").is_empty());
    }
}
