//! File / folder / chrome icons with three modes.
//!
//! Default is [`IconMode::Builtin`]: BMP Unicode that common fonts ship
//! (no Private Use Area → no tofu). Override with `FORGE_ICONS`:
//! - unset | `builtin` | `unicode` | `default` → Builtin
//! - `nerd` | `on` | `1` | `true` | `yes` | `nf` → Nerd (needs a Nerd Font)
//! - `off` | `0` | `ascii` | `none` | `false` | `no` → Ascii

use std::sync::atomic::{AtomicU8, Ordering};

const MODE_BUILTIN: u8 = 0;
const MODE_NERD: u8 = 1;
const MODE_ASCII: u8 = 2;

static ICON_MODE: AtomicU8 = AtomicU8::new(MODE_BUILTIN);

/// Which glyph set forge emits for file trees and chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconMode {
    /// Single-cell Unicode from the BMP (▸▾, □▣, ·, ⎇). Works without Nerd Font.
    #[default]
    Builtin,
    /// Nerd Font Private Use Area glyphs. Opt-in only.
    Nerd,
    /// Pure ASCII (`>`, `v`, `/`). Maximum compatibility.
    Ascii,
}

impl IconMode {
    /// Parse a `FORGE_ICONS` value. Unknown / empty → Builtin (never emit PUA by default).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "ascii" | "none" | "false" | "no" => IconMode::Ascii,
            "nerd" | "on" | "1" | "true" | "yes" | "nf" => IconMode::Nerd,
            "builtin" | "unicode" | "default" | "auto" | "" => IconMode::Builtin,
            _ => IconMode::Builtin,
        }
    }

    pub fn is_nerd(self) -> bool {
        matches!(self, IconMode::Nerd)
    }

    pub fn is_ascii(self) -> bool {
        matches!(self, IconMode::Ascii)
    }

    pub fn is_builtin(self) -> bool {
        matches!(self, IconMode::Builtin)
    }

    /// Short label for stderr / status (`builtin` | `nerd` | `ascii`).
    pub fn as_str(self) -> &'static str {
        match self {
            IconMode::Builtin => "builtin",
            IconMode::Nerd => "nerd",
            IconMode::Ascii => "ascii",
        }
    }
}

/// Install the icon mode used by every helper below.
pub fn set_icon_mode(mode: IconMode) {
    let v = match mode {
        IconMode::Builtin => MODE_BUILTIN,
        IconMode::Nerd => MODE_NERD,
        IconMode::Ascii => MODE_ASCII,
    };
    ICON_MODE.store(v, Ordering::Relaxed);
}

/// Current icon emission mode.
pub fn icon_mode() -> IconMode {
    match ICON_MODE.load(Ordering::Relaxed) {
        MODE_NERD => IconMode::Nerd,
        MODE_ASCII => IconMode::Ascii,
        _ => IconMode::Builtin,
    }
}

/// Read `FORGE_ICONS` and install the mode. Call once at startup.
/// Unset env → Builtin (safe without a Nerd Font).
pub fn init_from_env() {
    let mode = match std::env::var("FORGE_ICONS") {
        Ok(v) => IconMode::parse(&v),
        Err(_) => IconMode::Builtin,
    };
    set_icon_mode(mode);
}

/// Compact API used by widgets.
#[derive(Debug, Clone, Copy)]
pub struct Icons;

impl Icons {
    pub fn mode() -> IconMode {
        icon_mode()
    }

    /// Collapsed directory chevron.
    pub fn chevron_collapsed() -> &'static str {
        match icon_mode() {
            // nf-fa-chevron_right
            IconMode::Nerd => "\u{f054}",
            // BLACK RIGHT-POINTING SMALL TRIANGLE (BMP, usually 1 cell)
            IconMode::Builtin => "\u{25b8}",
            IconMode::Ascii => ">",
        }
    }

    pub fn chevron_expanded() -> &'static str {
        match icon_mode() {
            // nf-fa-chevron_down
            IconMode::Nerd => "\u{f078}",
            // BLACK DOWN-POINTING SMALL TRIANGLE
            IconMode::Builtin => "\u{25be}",
            IconMode::Ascii => "v",
        }
    }

    pub fn folder(open: bool) -> &'static str {
        match (icon_mode(), open) {
            // nf-fa-folder_open / nf-fa-folder
            (IconMode::Nerd, true) => "\u{f07c}",
            (IconMode::Nerd, false) => "\u{f07b}",
            // WHITE SQUARE CONTAINING BLACK SMALL SQUARE / WHITE SQUARE
            (IconMode::Builtin, true) => "\u{25a3}",
            (IconMode::Builtin, false) => "\u{25a1}",
            (IconMode::Ascii, _) => "/",
        }
    }

    pub fn folder_closed() -> &'static str {
        Self::folder(false)
    }

    /// Icon for a file basename (extension / special names).
    pub fn file_for_name(name: &str) -> &'static str {
        match icon_mode() {
            IconMode::Ascii => " ",
            // MIDDLE DOT — single cell, ubiquitous in Latin/CJK fonts
            IconMode::Builtin => "\u{00b7}",
            IconMode::Nerd => nerd_file_for_name(name),
        }
    }

    /// Directory or file icon for a list row.
    pub fn entry(is_dir: bool, name: &str) -> &'static str {
        if is_dir {
            Self::folder_closed()
        } else {
            Self::file_for_name(name)
        }
    }

    /// Git branch glyph for the header line.
    pub fn branch() -> &'static str {
        match icon_mode() {
            // nf-fa-code-branch
            IconMode::Nerd => "\u{f126}",
            // ALTERNATIVE KEY SYMBOL (⎇) — common “branch-ish” BMP glyph
            IconMode::Builtin => "\u{2387}",
            IconMode::Ascii => "",
        }
    }

    /// Leading gutter for a file-list row: chevron (dirs) or spacer (files).
    pub fn list_chevron(is_dir: bool) -> &'static str {
        if is_dir {
            Self::chevron_collapsed()
        } else {
            " "
        }
    }

    /// `"icon "` padded token so columns stay aligned.
    pub fn padded(icon: &str) -> String {
        if icon.is_empty() {
            "  ".to_string()
        } else {
            format!("{icon} ")
        }
    }
}

fn nerd_file_for_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let basename = lower.as_str();

    // Special basenames first.
    if basename == "cargo.toml" || basename == "rust-toolchain.toml" || basename.ends_with(".toml")
    {
        // nf-fa-cog
        return "\u{f013}";
    }
    if basename == "cargo.lock" || basename.ends_with(".lock") {
        // nf-fa-lock
        return "\u{f023}";
    }
    if basename == "license"
        || basename == "license.md"
        || basename == "license.txt"
        || basename.starts_with("license")
    {
        // nf-fa-balance_scale / book-ish: nf-fa-file_text_o
        return "\u{f0f6}";
    }
    if basename == ".gitignore"
        || basename == ".gitattributes"
        || basename == ".gitmodules"
        || basename.starts_with(".git")
    {
        // nf-dev-git
        return "\u{e702}";
    }
    if basename == "readme" || basename == "readme.md" || basename.starts_with("readme.") {
        // nf-dev-markdown
        return "\u{e73e}";
    }

    // Extension mapping.
    if let Some(ext) = basename.rsplit('.').next() {
        if ext != basename {
            match ext {
                "rs" => return "\u{e7a8}",       // nf-dev-rust
                "md" | "markdown" => return "\u{e73e}", // nf-dev-markdown
                "toml" => return "\u{f013}",     // cog
                "json" | "jsonc" => return "\u{e60b}", // nf-seti-json-ish / nf-mdi
                "yml" | "yaml" => return "\u{f481}", // nf-fa-file_code_o alt
                "lock" => return "\u{f023}",
                "sh" | "bash" | "zsh" => return "\u{f489}", // nf-fa-terminal-ish
                "py" => return "\u{e73c}",        // nf-dev-python
                "js" | "mjs" | "cjs" => return "\u{e74e}", // nf-dev-javascript
                "ts" | "tsx" => return "\u{e628}", // nf-seti-typescript
                "html" | "htm" => return "\u{e736}",
                "css" | "scss" => return "\u{e749}",
                "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" => return "\u{f1c5}",
                "txt" | "log" => return "\u{f15c}",
                "gitignore" => return "\u{e702}",
                _ => {}
            }
        }
    }

    // nf-fa-file
    "\u{f15b}"
}

/// True if `s` contains a Private Use Area codepoint (Nerd Font range heuristic).
#[allow(dead_code)]
pub fn contains_private_use(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        (0xE000..=0xF8FF).contains(&u)
            || (0xF0000..=0xFFFFD).contains(&u)
            || (0x100000..=0x10FFFD).contains(&u)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_icon_mode() {
        assert_eq!(IconMode::parse("nerd"), IconMode::Nerd);
        assert_eq!(IconMode::parse("ON"), IconMode::Nerd);
        assert_eq!(IconMode::parse("1"), IconMode::Nerd);
        assert_eq!(IconMode::parse("ascii"), IconMode::Ascii);
        assert_eq!(IconMode::parse("off"), IconMode::Ascii);
        assert_eq!(IconMode::parse("0"), IconMode::Ascii);
        assert_eq!(IconMode::parse("none"), IconMode::Ascii);
        assert_eq!(IconMode::parse(""), IconMode::Builtin);
        assert_eq!(IconMode::parse("builtin"), IconMode::Builtin);
        assert_eq!(IconMode::parse("unicode"), IconMode::Builtin);
        assert_eq!(IconMode::parse("default"), IconMode::Builtin);
        assert_eq!(IconMode::parse("wat"), IconMode::Builtin);
        assert_eq!(IconMode::default(), IconMode::Builtin);
    }

    #[test]
    fn file_for_name_mappings_nerd() {
        set_icon_mode(IconMode::Nerd);
        assert_eq!(Icons::file_for_name("main.rs"), "\u{e7a8}");
        assert_eq!(Icons::file_for_name("Cargo.toml"), "\u{f013}");
        assert_eq!(Icons::file_for_name("README.md"), "\u{e73e}");
        assert_eq!(Icons::file_for_name(".gitignore"), "\u{e702}");
        assert_eq!(Icons::file_for_name("Cargo.lock"), "\u{f023}");
        assert_eq!(Icons::file_for_name("LICENSE"), "\u{f0f6}");
        assert_eq!(Icons::file_for_name("notes.txt"), "\u{f15c}");
        assert_eq!(Icons::file_for_name("mystery.bin"), "\u{f15b}");
        assert!(contains_private_use(Icons::file_for_name("main.rs")));
        assert!(contains_private_use(Icons::folder_closed()));
        set_icon_mode(IconMode::Builtin);
    }

    #[test]
    fn ascii_mode_has_no_private_use() {
        set_icon_mode(IconMode::Ascii);
        assert_eq!(Icons::file_for_name("main.rs"), " ");
        assert_eq!(Icons::folder_closed(), "/");
        assert_eq!(Icons::chevron_collapsed(), ">");
        assert_eq!(Icons::branch(), "");
        assert!(!contains_private_use(Icons::file_for_name("Cargo.toml")));
        assert!(!contains_private_use(Icons::folder(true)));
        assert!(!contains_private_use(Icons::chevron_expanded()));
        set_icon_mode(IconMode::Builtin);
    }

    #[test]
    fn builtin_mode_no_private_use_and_readable() {
        set_icon_mode(IconMode::Builtin);
        assert_eq!(Icons::chevron_collapsed(), "\u{25b8}"); // ▸
        assert_eq!(Icons::chevron_expanded(), "\u{25be}"); // ▾
        assert_eq!(Icons::folder(false), "\u{25a1}"); // □
        assert_eq!(Icons::folder(true), "\u{25a3}"); // ▣
        assert_eq!(Icons::file_for_name("main.rs"), "\u{00b7}"); // ·
        assert_eq!(Icons::branch(), "\u{2387}"); // ⎇
        assert!(!contains_private_use(Icons::chevron_collapsed()));
        assert!(!contains_private_use(Icons::folder_closed()));
        assert!(!contains_private_use(Icons::file_for_name("x.rs")));
        assert!(!contains_private_use(Icons::branch()));
        assert_eq!(IconMode::Builtin.as_str(), "builtin");
    }

    #[test]
    fn padded_keeps_trailing_space() {
        set_icon_mode(IconMode::Ascii);
        assert_eq!(Icons::padded(Icons::folder_closed()), "/ ");
        assert_eq!(Icons::padded(""), "  ");
        set_icon_mode(IconMode::Builtin);
        assert_eq!(Icons::padded(Icons::folder_closed()), "\u{25a1} ");
    }
}
