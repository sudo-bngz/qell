//! Languages, syntax highlighting and colour themes.
//!
//! A [`Language`] bundles everything Qell needs to know about one language:
//! its id (`"rust"`), file extensions, tree-sitter grammar and queries, and
//! the command to launch its language server. The [`LanguageRegistry`]
//! holds the compiled tree-sitter configurations and produces
//! [`HighlightSpan`]s for a piece of text.
//!
//! Highlight capture names (`@keyword`, `@string`, …) are normalised into a
//! small [`TokenKind`] enum so the UI only has to map a dozen kinds to
//! colours instead of tracking every grammar's naming quirks.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Language as TsLanguage;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we recognise, in priority order. The index in this list is
/// what tree-sitter-highlight hands back, so [`TokenKind::from_index`] must
/// stay in sync with it.
const CAPTURE_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "function.special",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// The coarse token categories the UI colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Attribute,
    Comment,
    Constant,
    Function,
    Keyword,
    Label,
    Number,
    Operator,
    Property,
    Punctuation,
    String,
    Type,
    Variable,
    Parameter,
    Text,
}

impl TokenKind {
    fn from_index(i: usize) -> TokenKind {
        use TokenKind::*;
        match CAPTURE_NAMES.get(i).copied().unwrap_or("") {
            "attribute" => Attribute,
            "comment" | "comment.documentation" => Comment,
            "constant" | "constant.builtin" | "escape" => Constant,
            "constructor" => Type,
            "function" | "function.builtin" | "function.macro" | "function.method" | "function.special" => Function,
            "keyword" => Keyword,
            "label" => Label,
            "number" => Number,
            "operator" => Operator,
            "property" => Property,
            "punctuation.bracket" | "punctuation.delimiter" | "punctuation.special" => Punctuation,
            "string" | "string.special" => String,
            "type" | "type.builtin" => Type,
            "variable" | "variable.builtin" => Variable,
            "variable.parameter" => Parameter,
            _ => Text,
        }
    }
}

/// A highlighted byte range of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Static description of a language.
#[derive(Debug, Clone)]
pub struct Language {
    /// Stable identifier, also the LSP `languageId`.
    pub id: &'static str,
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Command line of the language server, if one is known.
    pub lsp: Option<&'static [&'static str]>,
    grammar: Option<Grammar>,
}

#[derive(Clone)]
struct Grammar {
    language: TsLanguage,
    highlights: &'static str,
    injections: &'static str,
    locals: &'static str,
}

impl std::fmt::Debug for Grammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Grammar")
    }
}

/// Every language Qell ships with. Adding a language is adding one entry
/// here (plus a grammar crate in `Cargo.toml`).
pub fn builtin_languages() -> Vec<Language> {
    vec![
        Language {
            id: "rust",
            name: "Rust",
            extensions: &["rs"],
            lsp: Some(&["rust-analyzer"]),
            grammar: Some(Grammar {
                language: tree_sitter_rust::LANGUAGE.into(),
                highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
                injections: tree_sitter_rust::INJECTIONS_QUERY,
                locals: "",
            }),
        },
        Language {
            id: "c",
            name: "C",
            extensions: &["c", "h"],
            lsp: Some(&["clangd"]),
            grammar: Some(Grammar {
                language: tree_sitter_c::LANGUAGE.into(),
                highlights: tree_sitter_c::HIGHLIGHT_QUERY,
                injections: "",
                locals: "",
            }),
        },
        Language {
            id: "go",
            name: "Go",
            extensions: &["go"],
            lsp: Some(&["gopls"]),
            grammar: Some(Grammar {
                language: tree_sitter_go::LANGUAGE.into(),
                highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
                injections: "",
                locals: "",
            }),
        },
        Language {
            id: "javascript",
            name: "JavaScript",
            extensions: &["js", "mjs", "cjs", "jsx"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(Grammar {
                language: tree_sitter_javascript::LANGUAGE.into(),
                highlights: tree_sitter_javascript::HIGHLIGHT_QUERY,
                injections: tree_sitter_javascript::INJECTIONS_QUERY,
                locals: tree_sitter_javascript::LOCALS_QUERY,
            }),
        },
        Language {
            id: "typescript",
            name: "TypeScript",
            extensions: &["ts", "mts", "cts"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(Grammar {
                language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
                injections: "",
                locals: tree_sitter_typescript::LOCALS_QUERY,
            }),
        },
        Language {
            id: "typescriptreact",
            name: "TSX",
            extensions: &["tsx"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(Grammar {
                language: tree_sitter_typescript::LANGUAGE_TSX.into(),
                highlights: tree_sitter_typescript::HIGHLIGHTS_QUERY,
                injections: "",
                locals: tree_sitter_typescript::LOCALS_QUERY,
            }),
        },
        Language {
            id: "terraform",
            name: "Terraform / HCL",
            extensions: &["tf", "tfvars", "hcl"],
            lsp: Some(&["terraform-ls", "serve"]),
            grammar: Some(Grammar {
                language: tree_sitter_hcl::LANGUAGE.into(),
                highlights: include_str!("../queries/hcl-highlights.scm"),
                injections: "",
                locals: "",
            }),
        },
        // VCL (Varnish) has no maintained tree-sitter grammar yet; it is
        // recognised so files open with the right language id and an LSP
        // can be attached, but it is rendered as plain text for now.
        Language { id: "vcl", name: "VCL", extensions: &["vcl"], lsp: None, grammar: None },
        Language { id: "plaintext", name: "Plain Text", extensions: &["txt", "md", "log"], lsp: None, grammar: None },
    ]
}

/// Language id for a file path, by extension.
pub fn language_for_path(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    builtin_languages().into_iter().find(|l| l.extensions.contains(&ext.as_str())).map(|l| l.id.to_string())
}

/// Compiled grammars plus a highlighter. One per editor is plenty; it is
/// cheap to create but compiling the queries takes a few milliseconds per
/// language, so keep it around.
pub struct LanguageRegistry {
    languages: Vec<Language>,
    configs: HashMap<&'static str, HighlightConfiguration>,
    highlighter: Highlighter,
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageRegistry {
    pub fn new() -> Self {
        let languages = builtin_languages();
        let mut configs = HashMap::new();
        for lang in &languages {
            let Some(g) = &lang.grammar else { continue };
            match HighlightConfiguration::new(g.language.clone(), lang.id, g.highlights, g.injections, g.locals) {
                Ok(mut cfg) => {
                    cfg.configure(CAPTURE_NAMES);
                    configs.insert(lang.id, cfg);
                }
                Err(e) => log::error!("failed to compile highlight query for {}: {e}", lang.id),
            }
        }
        Self { languages, configs, highlighter: Highlighter::new() }
    }

    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    pub fn get(&self, id: &str) -> Option<&Language> {
        self.languages.iter().find(|l| l.id == id)
    }

    /// Highlight `source` as language `lang_id`. Unknown or grammar-less
    /// languages return an empty list, which the UI renders as plain text.
    pub fn highlight(&mut self, lang_id: &str, source: &str) -> Vec<HighlightSpan> {
        let Some(cfg) = self.configs.get(lang_id) else { return Vec::new() };
        let configs = &self.configs;
        let events = match self.highlighter.highlight(cfg, source.as_bytes(), None, None, |name| configs.get(name)) {
            Ok(ev) => ev,
            Err(e) => {
                log::warn!("highlight error for {lang_id}: {e}");
                return Vec::new();
            }
        };

        let mut spans = Vec::new();
        let mut stack: Vec<TokenKind> = Vec::new();
        for ev in events.flatten() {
            match ev {
                HighlightEvent::HighlightStart(h) => stack.push(TokenKind::from_index(h.0)),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(kind) = stack.last().copied() {
                        if kind != TokenKind::Text {
                            spans.push(HighlightSpan { start, end, kind });
                        }
                    }
                }
            }
        }
        spans
    }
}

/// An RGB colour theme for tokens. Lives in core (not the UI crate) so
/// themes can be loaded from config and shared with the terminal palette.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub colors: HashMap<TokenKind, [u8; 3]>,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub gutter: [u8; 3],
    pub selection: [u8; 3],
    pub cursor: [u8; 3],
    pub current_line: [u8; 3],
}

impl Theme {
    /// A "Dark Modern"-style palette close to VSCodium's default.
    pub fn dark() -> Self {
        use TokenKind::*;
        let colors = HashMap::from([
            (Attribute, [0xdc, 0xdc, 0xaa]),
            (Comment, [0x6a, 0x99, 0x55]),
            (Constant, [0x4f, 0xc1, 0xff]),
            (Function, [0xdc, 0xdc, 0xaa]),
            (Keyword, [0xc5, 0x86, 0xc0]),
            (Label, [0xc8, 0xc8, 0xc8]),
            (Number, [0xb5, 0xce, 0xa8]),
            (Operator, [0xd4, 0xd4, 0xd4]),
            (Property, [0x9c, 0xdc, 0xfe]),
            (Punctuation, [0xd4, 0xd4, 0xd4]),
            (String, [0xce, 0x91, 0x78]),
            (Type, [0x4e, 0xc9, 0xb0]),
            (Variable, [0x9c, 0xdc, 0xfe]),
            (Parameter, [0x9c, 0xdc, 0xfe]),
            (Text, [0xd4, 0xd4, 0xd4]),
        ]);
        Self {
            name: "Qell Dark".into(),
            colors,
            foreground: [0xcc, 0xcc, 0xcc],
            background: [0x1f, 0x1f, 0x1f],
            gutter: [0x6e, 0x76, 0x81],
            selection: [0x26, 0x4f, 0x78],
            cursor: [0xae, 0xaf, 0xad],
            current_line: [0x28, 0x28, 0x28],
        }
    }

    pub fn color(&self, kind: TokenKind) -> [u8; 3] {
        self.colors.get(&kind).copied().unwrap_or(self.foreground)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grammar_compiles() {
        let reg = LanguageRegistry::new();
        for lang in reg.languages() {
            if lang.grammar.is_some() {
                assert!(reg.configs.contains_key(lang.id), "{} did not compile", lang.id);
            }
        }
    }

    #[test]
    fn rust_keywords_are_highlighted() {
        let mut reg = LanguageRegistry::new();
        let src = "fn main() { let x = 1; }";
        let spans = reg.highlight("rust", src);
        let kw = spans.iter().find(|s| &src[s.start..s.end] == "fn").expect("fn span");
        assert_eq!(kw.kind, TokenKind::Keyword);
        let num = spans.iter().find(|s| &src[s.start..s.end] == "1").expect("number span");
        // The Rust grammar captures integer literals as @constant.
        assert!(matches!(num.kind, TokenKind::Number | TokenKind::Constant));
    }

    #[test]
    fn terraform_blocks_are_highlighted() {
        let mut reg = LanguageRegistry::new();
        let src = "resource \"aws_instance\" \"web\" {\n  ami = \"abc\"\n}\n";
        let spans = reg.highlight("terraform", src);
        assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword && &src[s.start..s.end] == "resource"));
        assert!(spans.iter().any(|s| s.kind == TokenKind::String));
    }

    #[test]
    fn extension_lookup() {
        assert_eq!(language_for_path(Path::new("a/b.rs")).as_deref(), Some("rust"));
        assert_eq!(language_for_path(Path::new("main.tf")).as_deref(), Some("terraform"));
        assert_eq!(language_for_path(Path::new("x.tsx")).as_deref(), Some("typescriptreact"));
        assert_eq!(language_for_path(Path::new("default.vcl")).as_deref(), Some("vcl"));
        assert_eq!(language_for_path(Path::new("noext")), None);
    }
}
