//! File-type icons for the file tree and tabs.
//!
//! The `devicons` crate picks a Nerd Fonts glyph and brand colour per file
//! (Terraform purple, Rust orange, …); the glyphs come from a small subset
//! of Symbols Nerd Font bundled in `assets/fonts` and registered as the
//! [`family`] font family.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};

const FONT_NAME: &str = "qell-icons";
const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/qell-icons.ttf");

pub const FOLDER: char = '\u{e5ff}';
pub const FOLDER_OPEN: char = '\u{e5fe}';
/// Codicons, for the activity bar and Source Control.
pub const EXPLORER: char = '\u{eaf0}';
pub const SOURCE_CONTROL: char = '\u{ea68}';
pub const ADD: char = '\u{ea60}';
pub const REMOVE: char = '\u{eb3b}';
pub const CHECK: char = '\u{eab2}';
pub const REFRESH: char = '\u{eb37}';
pub const SEARCH: char = '\u{ea6d}';
pub const CASE_SENSITIVE: char = '\u{eab1}';
pub const WHOLE_WORD: char = '\u{eb7e}';
pub const REGEX: char = '\u{eb38}';
pub const CLOSE: char = '\u{ea76}';
pub const CHEVRON_RIGHT: char = '\u{eab6}';
pub const CHEVRON_DOWN: char = '\u{eab4}';

/// Unknown file types (devicons' own fallback is a plain `*`).
const DEFAULT_FILE: (char, Color32) = ('\u{e64e}', Color32::from_rgb(0x7e, 0x8e, 0xa8));

/// Types devicons has no icon for, by lower-cased extension.
const OVERRIDES: &[(&str, char, Color32)] = &[
    // VCL (Varnish): config gear in Varnish's blue.
    ("vcl", '\u{e615}', Color32::from_rgb(0x2b, 0x9a, 0xd6)),
];

/// The font family icons are drawn with. Falls back to the default fonts
/// for the few plain-Unicode symbols (∞, λ, ⚙) devicons also uses.
pub fn family() -> FontFamily {
    FontFamily::Name(FONT_NAME.into())
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, family())
}

/// Fonts already on the machine, tried in order for characters the
/// bundled ones lack: shell prompts print `✗`, `✔`, `⚡`, box drawing
/// and arrows that Hack doesn't have, which showed as empty squares.
/// Read from disk once at startup if present; nothing is downloaded.
const SYSTEM_FALLBACKS: &[&str] = &[
    // macOS
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Apple Symbols.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    // Windows
    "C:\\Windows\\Fonts\\seguisym.ttf",
    "C:\\Windows\\Fonts\\consola.ttf",
];

/// Add the system fallbacks that exist after the fonts of `family`.
fn add_system_fallbacks(fonts: &mut FontDefinitions, family: FontFamily) {
    for path in SYSTEM_FALLBACKS {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let name = format!("system:{path}");
        // The first face of a collection (.ttc) is the regular one.
        fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(family.clone()).or_default().push(name);
    }
}

/// Register the icon font with egui. Call once at startup.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(FONT_NAME.into(), Arc::new(FontData::from_static(FONT_BYTES)));
    add_system_fallbacks(&mut fonts, FontFamily::Monospace);
    let mut chain = vec![FONT_NAME.to_string()];
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        for f in fonts.families.get(&fam).cloned().unwrap_or_default() {
            if !chain.contains(&f) {
                chain.push(f);
            }
        }
    }
    fonts.families.insert(family(), chain);
    // One typeface: menus, tabs and file names are set in the same
    // monospace as the code and the terminal.
    if let Some(mono) = fonts.families.get(&FontFamily::Monospace).cloned() {
        fonts.families.insert(FontFamily::Proportional, mono);
    }
    ctx.set_fonts(fonts);
}

fn parse_hex(hex: &str) -> Option<Color32> {
    let h = hex.strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    (h.len() == 6).then(|| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

thread_local! {
    static CACHE: RefCell<HashMap<String, (char, Color32)>> = RefCell::new(HashMap::new());
}

/// Icon and colour for a file. Cached by file name: devicons only looks at
/// the name and extension, and the tree asks every frame.
pub fn for_file(path: &Path) -> (char, Color32) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(&name) {
            return *hit;
        }
        let entry = lookup(&name);
        cache.borrow_mut().insert(name, entry);
        entry
    })
}

fn lookup(name: &str) -> (char, Color32) {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    if let Some((_, c, col)) = OVERRIDES.iter().find(|(e, _, _)| Some(*e) == ext.as_deref()) {
        return (*c, *col);
    }
    // devicons matches names case-sensitively and lists `makefile`,
    // `license`, … in lower case, so try that too. Look up under a dummy
    // directory: for unknown names devicons stats the path, and this one
    // never exists.
    for candidate in [name.to_string(), name.to_ascii_lowercase()] {
        let probe = Path::new("/nonexistent-qell-icon-probe").join(&candidate);
        let icon = devicons::icon_for_file(probe.as_path(), &Some(devicons::Theme::Dark));
        // devicons' fallback (not exported) is a plain '*'.
        if icon.icon != '*' {
            return (icon.icon, parse_hex(icon.color).unwrap_or(Color32::GRAY));
        }
    }
    DEFAULT_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_icons_differ_by_type() {
        let tf = for_file(Path::new("main.tf"));
        let rs = for_file(Path::new("src/lib.rs"));
        let json = for_file(Path::new("package.json"));
        let docker = for_file(Path::new("Dockerfile"));
        let unknown = for_file(Path::new("whatever.qellunknown"));
        assert_ne!(tf.0, rs.0);
        assert_ne!(rs.0, json.0);
        assert_ne!(docker.0, unknown.0);
        assert_ne!(tf.0, unknown.0);
    }

    #[test]
    fn system_fallbacks_come_after_the_bundled_fonts() {
        let mut fonts = FontDefinitions::default();
        let before = fonts.families[&FontFamily::Monospace].clone();
        add_system_fallbacks(&mut fonts, FontFamily::Monospace);
        let after = &fonts.families[&FontFamily::Monospace];
        // The bundled ones keep the lead; anything found is only behind them.
        assert_eq!(after[..before.len()], before[..]);
        assert!(after[before.len()..].iter().all(|n| n.starts_with("system:") && fonts.font_data.contains_key(n)));
    }

    #[test]
    fn case_insensitive_names_and_overrides() {
        let make = for_file(Path::new("Makefile"));
        assert_ne!(make, DEFAULT_FILE);
        assert_ne!(for_file(Path::new("LICENSE")), DEFAULT_FILE);
        assert_eq!(for_file(Path::new("default.vcl")).0, '\u{e615}');
        assert_eq!(for_file(Path::new("x.qellunknown")), DEFAULT_FILE);
    }
}
