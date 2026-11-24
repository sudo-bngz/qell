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
use std::sync::LazyLock;

use tree_sitter::Language as TsLanguage;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we recognise, in priority order. The index in this list is
/// what tree-sitter-highlight hands back, so [`TokenKind::from_index`] must
/// stay in sync with it. A capture is matched to the entry sharing the most
/// dot-separated parts, so `@keyword.conditional` falls back to `keyword`.
const CAPTURE_NAMES: &[&str] = &[
    "attribute",
    "boolean",
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
    "string.escape",
    "string.special",
    "string.special.key",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    // Markup (Markdown, XML). Older grammars use `text.*`, newer `markup.*`.
    "text.title",
    "markup.heading",
    "text.emphasis",
    "markup.italic",
    "text.strong",
    "markup.bold",
    "text.literal",
    "markup.raw",
    "text.uri",
    "text.reference",
    "markup.link",
    // Older nvim-style names still used by some grammars (Makefile, CSS).
    "conditional",
    "repeat",
    "include",
    "exception",
    "import",
    "media",
    "keyframes",
    "supports",
    "namespace",
    "charset",
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
    /// HTML / XML element names.
    Tag,
    /// Markdown headings.
    Heading,
    /// Italic text (rendered in italics).
    Emphasis,
    /// Bold text.
    Strong,
    /// URLs and link targets (rendered underlined).
    Link,
    Text,
}

impl TokenKind {
    pub const ALL: &'static [TokenKind] = {
        use TokenKind::*;
        &[
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
            Tag,
            Heading,
            Emphasis,
            Strong,
            Link,
            Text,
        ]
    };

    fn from_index(i: usize) -> TokenKind {
        use TokenKind::*;
        match CAPTURE_NAMES.get(i).copied().unwrap_or("") {
            "attribute" => Attribute,
            "comment" | "comment.documentation" => Comment,
            "boolean" | "constant" | "constant.builtin" | "escape" | "string.escape" => Constant,
            "constructor" => Type,
            "function" | "function.builtin" | "function.macro" | "function.method" | "function.special" => Function,
            "keyword" | "conditional" | "repeat" | "include" | "exception" | "import" | "media" | "keyframes" | "supports"
            | "namespace" | "charset" => Keyword,
            "label" => Label,
            "number" => Number,
            "operator" => Operator,
            "property" | "string.special.key" => Property,
            "punctuation.bracket" | "punctuation.delimiter" | "punctuation.special" => Punctuation,
            "string" | "string.special" | "text.literal" | "markup.raw" => String,
            "tag" => Tag,
            "type" | "type.builtin" => Type,
            "variable" | "variable.builtin" => Variable,
            "variable.parameter" => Parameter,
            "text.title" | "markup.heading" => Heading,
            "text.emphasis" | "markup.italic" => Emphasis,
            "text.strong" | "markup.bold" => Strong,
            "text.uri" | "text.reference" | "markup.link" => Link,
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
#[derive(Debug, Clone, Default)]
pub struct Language {
    /// Stable identifier, also the LSP `languageId`.
    pub id: &'static str,
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Exact file names (`Makefile`, `.bashrc`) for files without a telling
    /// extension.
    pub filenames: &'static [&'static str],
    /// Other names the language goes by, used to resolve injections such as
    /// a Markdown code fence tagged `sh` or `yml`.
    pub aliases: &'static [&'static str],
    /// Command line of the language server, if one is known.
    pub lsp: Option<&'static [&'static str]>,
    /// Hand-written highlighter for languages without a tree-sitter grammar.
    pub lexer: Option<fn(&str) -> Vec<HighlightSpan>>,
    /// Internal languages (Markdown's inline grammar) are only reachable
    /// through injections, never picked for a file.
    pub hidden: bool,
    grammar: Option<Grammar>,
}

#[derive(Clone)]
struct Grammar {
    language: TsLanguage,
    highlights: &'static str,
    injections: &'static str,
    locals: &'static str,
}

impl Grammar {
    fn new(language: TsLanguage, highlights: &'static str) -> Self {
        Self { language, highlights, injections: "", locals: "" }
    }

    fn injections(mut self, q: &'static str) -> Self {
        self.injections = q;
        self
    }

    fn locals(mut self, q: &'static str) -> Self {
        self.locals = q;
        self
    }
}

impl std::fmt::Debug for Grammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Grammar")
    }
}

/// Upstream Makefile highlights plus Qell's additions.
static MAKE_HIGHLIGHTS: LazyLock<String> = LazyLock::new(|| {
    format!("{}\n{}", tree_sitter_make::HIGHLIGHTS_QUERY, include_str!("../queries/makefile-highlights-extra.scm"))
});

/// Every language Qell ships with. Adding a language is adding one entry
/// here (plus a grammar crate in `Cargo.toml`).
pub fn builtin_languages() -> Vec<Language> {
    vec![
        Language {
            id: "rust",
            name: "Rust",
            extensions: &["rs"],
            aliases: &["rs"],
            lsp: Some(&["rust-analyzer"]),
            grammar: Some(
                Grammar::new(tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY)
                    .injections(tree_sitter_rust::INJECTIONS_QUERY),
            ),
            ..Default::default()
        },
        Language {
            id: "c",
            name: "C",
            extensions: &["c", "h"],
            lsp: Some(&["clangd"]),
            grammar: Some(Grammar::new(tree_sitter_c::LANGUAGE.into(), tree_sitter_c::HIGHLIGHT_QUERY)),
            ..Default::default()
        },
        Language {
            id: "go",
            name: "Go",
            extensions: &["go"],
            aliases: &["golang"],
            lsp: Some(&["gopls"]),
            grammar: Some(Grammar::new(tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY)),
            ..Default::default()
        },
        Language {
            id: "javascript",
            name: "JavaScript",
            extensions: &["js", "mjs", "cjs", "jsx"],
            aliases: &["js", "jsx", "node"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(
                Grammar::new(tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHT_QUERY)
                    .injections(tree_sitter_javascript::INJECTIONS_QUERY)
                    .locals(tree_sitter_javascript::LOCALS_QUERY),
            ),
            ..Default::default()
        },
        Language {
            id: "typescript",
            name: "TypeScript",
            extensions: &["ts", "mts", "cts"],
            aliases: &["ts"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(
                Grammar::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY)
                    .locals(tree_sitter_typescript::LOCALS_QUERY),
            ),
            ..Default::default()
        },
        Language {
            id: "typescriptreact",
            name: "TSX",
            extensions: &["tsx"],
            aliases: &["tsx"],
            lsp: Some(&["typescript-language-server", "--stdio"]),
            grammar: Some(
                Grammar::new(tree_sitter_typescript::LANGUAGE_TSX.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY)
                    .locals(tree_sitter_typescript::LOCALS_QUERY),
            ),
            ..Default::default()
        },
        Language {
            id: "terraform",
            name: "Terraform / HCL",
            extensions: &["tf", "tfvars", "hcl"],
            filenames: &[".terraformrc", "terraform.rc"],
            aliases: &["tf", "hcl"],
            lsp: Some(&["terraform-ls", "serve"]),
            grammar: Some(Grammar::new(tree_sitter_hcl::LANGUAGE.into(), include_str!("../queries/hcl-highlights.scm"))),
            ..Default::default()
        },
        Language {
            id: "vcl",
            name: "VCL",
            extensions: &["vcl"],
            aliases: &["varnish"],
            grammar: Some(Grammar::new(tree_sitter_vcl::LANGUAGE.into(), include_str!("../queries/vcl-highlights.scm"))),
            ..Default::default()
        },
        Language {
            id: "markdown",
            name: "Markdown",
            extensions: &["md", "markdown", "mdown", "mkd"],
            aliases: &["md"],
            grammar: Some(
                Grammar::new(tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)
                    .injections(include_str!("../queries/markdown-injections.scm")),
            ),
            ..Default::default()
        },
        Language {
            id: "markdown_inline",
            name: "Markdown (inline)",
            hidden: true,
            grammar: Some(
                Grammar::new(tree_sitter_md::INLINE_LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_INLINE)
                    .injections(tree_sitter_md::INJECTION_QUERY_INLINE),
            ),
            ..Default::default()
        },
        Language {
            id: "html",
            name: "HTML",
            extensions: &["html", "htm", "xhtml"],
            grammar: Some(
                Grammar::new(tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY)
                    .injections(tree_sitter_html::INJECTIONS_QUERY),
            ),
            ..Default::default()
        },
        Language {
            id: "css",
            name: "CSS",
            extensions: &["css"],
            grammar: Some(Grammar::new(tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY)),
            ..Default::default()
        },
        Language {
            id: "json",
            name: "JSON",
            extensions: &["json", "jsonc", "json5", "geojson", "webmanifest", "tfstate"],
            filenames: &[".babelrc", ".eslintrc", ".prettierrc", "flake.lock", "composer.lock"],
            grammar: Some(Grammar::new(tree_sitter_json::LANGUAGE.into(), include_str!("../queries/json-highlights.scm"))),
            ..Default::default()
        },
        Language {
            id: "toml",
            name: "TOML",
            extensions: &["toml"],
            filenames: &["Cargo.lock", "poetry.lock", "Pipfile", "uv.lock"],
            grammar: Some(Grammar::new(tree_sitter_toml_ng::LANGUAGE.into(), tree_sitter_toml_ng::HIGHLIGHTS_QUERY)),
            ..Default::default()
        },
        Language {
            id: "yaml",
            name: "YAML",
            extensions: &["yaml", "yml"],
            filenames: &[".clang-format", ".clang-tidy", ".clangd"],
            aliases: &["yml"],
            grammar: Some(Grammar::new(tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY)),
            ..Default::default()
        },
        Language {
            id: "xml",
            name: "XML",
            extensions: &["xml", "svg", "xsd", "xsl", "xslt", "plist", "csproj", "fsproj", "props", "targets", "rss", "atom"],
            grammar: Some(Grammar::new(tree_sitter_xml::LANGUAGE_XML.into(), tree_sitter_xml::XML_HIGHLIGHT_QUERY)),
            ..Default::default()
        },
        Language {
            id: "ini",
            name: "INI",
            extensions: &["ini", "cfg", "properties", "desktop", "service", "timer", "socket", "editorconfig"],
            filenames: &[".editorconfig", ".gitconfig", ".npmrc", ".pylintrc", "setup.cfg", "tox.ini", "config"],
            aliases: &["dosini", "gitconfig"],
            grammar: Some(Grammar::new(tree_sitter_ini::LANGUAGE.into(), tree_sitter_ini::HIGHLIGHTS_QUERY)),
            ..Default::default()
        },
        Language {
            id: "shellscript",
            name: "Shell Script",
            extensions: &["sh", "bash", "zsh", "ksh", "env", "envrc"],
            filenames: &[
                ".bashrc",
                ".bash_profile",
                ".bash_aliases",
                ".bash_logout",
                ".profile",
                ".zshrc",
                ".zshenv",
                ".zprofile",
                ".env",
                ".envrc",
                "PKGBUILD",
            ],
            aliases: &["sh", "bash", "zsh", "shell", "console", "shellsession"],
            grammar: Some(Grammar::new(tree_sitter_bash::LANGUAGE.into(), tree_sitter_bash::HIGHLIGHT_QUERY)),
            ..Default::default()
        },
        Language {
            id: "makefile",
            name: "Makefile",
            extensions: &["mk", "mak"],
            filenames: &["Makefile", "makefile", "GNUmakefile"],
            aliases: &["make"],
            grammar: Some(
                Grammar::new(tree_sitter_make::LANGUAGE.into(), &MAKE_HIGHLIGHTS)
                    .injections(include_str!("../queries/makefile-injections.scm")),
            ),
            ..Default::default()
        },
        Language {
            id: "nginx",
            name: "Nginx",
            extensions: &["nginx", "nginxconf"],
            filenames: &["nginx.conf", "mime.types", "fastcgi_params", "uwsgi_params", "scgi_params"],
            grammar: Some(Grammar::new(tree_sitter_nginx::LANGUAGE.into(), include_str!("../queries/nginx-highlights.scm"))),
            ..Default::default()
        },
        Language {
            id: "dockerfile",
            name: "Dockerfile",
            extensions: &["dockerfile", "containerfile"],
            filenames: &["Dockerfile", "Containerfile", "dockerfile"],
            aliases: &["docker"],
            grammar: Some(
                Grammar::new(arborium_dockerfile::language().into(), arborium_dockerfile::HIGHLIGHTS_QUERY)
                    .injections(include_str!("../queries/dockerfile-injections.scm")),
            ),
            ..Default::default()
        },
        Language {
            id: "conf",
            name: "Config",
            extensions: &["conf", "cnf", "cf", "config", "rc"],
            filenames: &["sshd_config", "ssh_config", "hosts", "fstab", "crontab", "inputrc", ".tmux.conf", ".wgetrc", ".curlrc"],
            aliases: &["apacheconf", "config"],
            lexer: Some(crate::conf::highlight),
            ..Default::default()
        },
        Language { id: "alea", name: "alea", extensions: &["alea"], lexer: Some(crate::alea::highlight), ..Default::default() },
        Language { id: "plaintext", name: "Plain Text", extensions: &["txt", "log"], ..Default::default() },
    ]
}

/// Language id for a file path: by exact file name first (`Makefile`,
/// `.bashrc`), then `.env.*` files, then by extension.
pub fn language_for_path(path: &Path) -> Option<String> {
    let languages = builtin_languages();
    let name = path.file_name()?.to_str()?;
    if let Some(l) = languages.iter().find(|l| l.filenames.contains(&name)) {
        return Some(l.id.to_string());
    }
    // .env.local, .env.production, …
    if name.starts_with(".env.") {
        return Some("shellscript".into());
    }
    // Dockerfile.dev, prod.Dockerfile, …
    if name.starts_with("Dockerfile.") || name.starts_with("Containerfile.") {
        return Some("dockerfile".into());
    }
    // *.conf under an nginx directory (sites-available/, conf.d/) is nginx.
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if ext.as_deref() == Some("conf") && path.components().any(|c| c.as_os_str().to_str().is_some_and(|c| c.contains("nginx"))) {
        return Some("nginx".into());
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    languages.into_iter().find(|l| !l.hidden && l.extensions.contains(&ext.as_str())).map(|l| l.id.to_string())
}

/// Compiled grammars plus a highlighter. One per editor is plenty; it is
/// cheap to create but compiling the queries takes a few milliseconds per
/// language, so keep it around.
pub struct LanguageRegistry {
    languages: Vec<Language>,
    configs: HashMap<&'static str, HighlightConfiguration>,
    /// Lower-cased id, alias or extension → language id, for injections.
    names: HashMap<String, &'static str>,
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
        let mut names = HashMap::new();
        for lang in &languages {
            // Ids win over aliases, aliases over extensions.
            for n in lang.extensions.iter().chain(lang.aliases).chain([&lang.id]) {
                names.insert(n.to_ascii_lowercase(), lang.id);
            }
        }
        Self { languages, configs, names, highlighter: Highlighter::new() }
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
        let Some(cfg) = self.configs.get(lang_id) else {
            return match self.get(lang_id).and_then(|l| l.lexer) {
                Some(lex) => lex(source),
                None => Vec::new(),
            };
        };
        let configs = &self.configs;
        let names = &self.names;
        // Injected languages (a ```sh fence, <script> in HTML) are named by
        // whatever the author wrote, so resolve aliases too.
        let injected = |name: &str| names.get(&name.trim().to_ascii_lowercase()).and_then(|id| configs.get(id));
        let events = match self.highlighter.highlight(cfg, source.as_bytes(), None, None, injected) {
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
    /// The dark theme: the familiar "Dark Modern" token colours, on the
    /// near-black surface the rest of the interface uses.
    pub fn dark() -> Self {
        use TokenKind::*;
        const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
        const FG: [u8; 3] = [0xe2, 0xde, 0xd8];
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
            (Tag, [0x56, 0x9c, 0xd6]),
            (Heading, [0x56, 0x9c, 0xd6]),
            (Emphasis, [0xd4, 0xd4, 0xd4]),
            (Strong, [0x56, 0x9c, 0xd6]),
            (Link, [0x4f, 0xc1, 0xff]),
            (Text, [0xd4, 0xd4, 0xd4]),
        ]);
        Self {
            name: "Qell Dark".into(),
            colors,
            foreground: FG,
            background: [0x11, 0x0f, 0x0e],
            gutter: [0x57, 0x51, 0x4c],
            selection: [0x3b, 0x36, 0x32],
            cursor: WHITE,
            current_line: [0x19, 0x17, 0x15],
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

    /// Kind of the first span covering exactly `token` in `src`.
    fn kind_of(reg: &mut LanguageRegistry, lang: &str, src: &str, token: &str) -> Option<TokenKind> {
        let spans = reg.highlight(lang, src);
        spans.iter().find(|s| src[s.start..s.end].trim() == token).map(|s| s.kind)
    }

    #[test]
    fn config_and_markup_languages_are_highlighted() {
        use TokenKind::*;
        let mut reg = LanguageRegistry::new();
        let cases: &[(&str, &str, &str, &[TokenKind])] = &[
            ("json", r#"{"name": "qell", "n": 1, "ok": true}"#, r#""name""#, &[Property]),
            ("json", r#"{"name": "qell", "n": 1, "ok": true}"#, "true", &[Constant]),
            ("toml", "[package]\nname = \"qell\" # c\n", "name", &[Property]),
            ("toml", "[package]\nname = \"qell\" # c\n", "# c", &[Comment]),
            ("yaml", "key: value\nlist:\n  - 3\n", "key", &[Property]),
            ("yaml", "key: value\nlist:\n  - 3\n", "3", &[Number]),
            ("html", "<div class=\"a\">hi</div>", "div", &[Tag]),
            ("html", "<div class=\"a\">hi</div>", "class", &[Attribute]),
            ("css", "body { color: red; }", "color", &[Property]),
            ("xml", "<root attr=\"1\"><a/></root>", "root", &[Tag]),
            ("ini", "[core]\neditor = vim\n", "editor", &[Property]),
            ("shellscript", "export FOO=bar # set\necho \"$FOO\"\n", "# set", &[Comment]),
            ("shellscript", "if true; then echo hi; fi\n", "if", &[Keyword]),
            ("shellscript", "PORT=8080\n", "PORT", &[Property, Variable]),
            ("makefile", "all: build\n\tcargo build\n", "all", &[Function]),
            ("makefile", "build: dep\n\tcargo build\n", "build", &[Function]),
            ("makefile", "build: dep\n\tif true; then cargo build; fi\n", "if", &[Keyword]),
            ("vcl", "vcl 4.1;\nsub vcl_recv {\n  return (pass);\n}\n", "sub", &[Keyword]),
            ("nginx", "server {\n  listen 80;\n}\n", "listen", &[Keyword, Property, Function, Variable]),
            ("markdown", "# Title\n\nSome *em* text.\n", "Title", &[Heading]),
            ("dockerfile", "FROM rust:1.95 AS build\nRUN if true; then cargo build; fi\n", "FROM", &[Keyword]),
            ("dockerfile", "FROM rust:1.95 AS build\nRUN if true; then cargo build; fi\n", "if", &[Keyword]),
            ("conf", "[mysqld]\nport = 3306\n", "port", &[Property]),
            ("markdown", "# Title\n\nSome *em* text.\n", "em", &[Emphasis]),
        ];
        let mut failures = Vec::new();
        for (lang, src, token, expected) in cases {
            let got = kind_of(&mut reg, lang, src, token);
            if !got.is_some_and(|k| expected.contains(&k)) {
                let spans: Vec<_> = reg.highlight(lang, src).iter().map(|s| (&src[s.start..s.end], s.kind)).collect();
                failures.push(format!("{lang}: {token:?} was {got:?}, expected {expected:?}; spans {spans:?}"));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn markdown_code_fences_use_the_fenced_language() {
        let mut reg = LanguageRegistry::new();
        // `sh` is an alias, not the language id.
        let src = "Run:\n\n```sh\nif true; then echo hi; fi\n```\n\n```rust\nfn main() {}\n```\n";
        assert_eq!(kind_of(&mut reg, "markdown", src, "if"), Some(TokenKind::Keyword));
        assert_eq!(kind_of(&mut reg, "markdown", src, "fn"), Some(TokenKind::Keyword));
    }

    #[test]
    fn html_script_is_javascript() {
        let mut reg = LanguageRegistry::new();
        let src = "<script>const x = 1;</script>";
        assert_eq!(kind_of(&mut reg, "html", src, "const"), Some(TokenKind::Keyword));
    }

    #[test]
    fn filename_lookup() {
        let id = |p: &str| language_for_path(Path::new(p));
        assert_eq!(id("Makefile").as_deref(), Some("makefile"));
        assert_eq!(id("/home/me/.bashrc").as_deref(), Some("shellscript"));
        assert_eq!(id(".env").as_deref(), Some("shellscript"));
        assert_eq!(id(".env.production").as_deref(), Some("shellscript"));
        assert_eq!(id("Cargo.lock").as_deref(), Some("toml"));
        assert_eq!(id("README.md").as_deref(), Some("markdown"));
        assert_eq!(id("docker-compose.yml").as_deref(), Some("yaml"));
        assert_eq!(id("tsconfig.json").as_deref(), Some("json"));
        assert_eq!(id("/etc/nginx/nginx.conf").as_deref(), Some("nginx"));
        assert_eq!(id(".gitconfig").as_deref(), Some("ini"));
        assert_eq!(id("Dockerfile").as_deref(), Some("dockerfile"));
        assert_eq!(id("Dockerfile.dev").as_deref(), Some("dockerfile"));
        assert_eq!(id("app.dockerfile").as_deref(), Some("dockerfile"));
        assert_eq!(id("/etc/nginx/sites-available/default.conf").as_deref(), Some("nginx"));
        assert_eq!(id("/etc/apache2/apache2.conf").as_deref(), Some("conf"));
        assert_eq!(id("/etc/ssh/sshd_config").as_deref(), Some("conf"));
        assert_eq!(id("my.cnf").as_deref(), Some("conf"));
    }

    #[test]
    fn extension_lookup() {
        assert_eq!(language_for_path(Path::new("a/b.rs")).as_deref(), Some("rust"));
        assert_eq!(language_for_path(Path::new("main.tf")).as_deref(), Some("terraform"));
        assert_eq!(language_for_path(Path::new("x.tsx")).as_deref(), Some("typescriptreact"));
        assert_eq!(language_for_path(Path::new("default.vcl")).as_deref(), Some("vcl"));
        assert_eq!(language_for_path(Path::new("rosace.alea")).as_deref(), Some("alea"));
        assert_eq!(language_for_path(Path::new("noext")), None);
    }
}
