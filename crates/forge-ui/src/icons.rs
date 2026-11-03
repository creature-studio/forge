//! File / folder / chrome icons with Nerd Font glyphs and ASCII fallback.
//!
//! Default is [`IconMode::Nerd`]. Override with `FORGE_ICONS`:
//! - `nerd` | `on` | `1` → Nerd
//! - `off` | `0` | `ascii` | `none` → Ascii

use std::sync::atomic::{AtomicU8, Ordering};

const MODE_NERD: u8 = 0;
const MODE_ASCII: u8 = 1;

static ICON_MODE: AtomicU8 = AtomicU8::new(MODE_NERD);

/// Which glyph set forge emits for file trees and chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconMode {
    #[default]
    Nerd,
    Ascii,
}

impl IconMode {
    /// Parse a `FORGE_ICONS` value. Unknown values fall back to Nerd.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "ascii" | "none" | "false" | "no" => IconMode::Ascii,
            "nerd" | "on" | "1" | "true" | "yes" | "nf" => IconMode::Nerd,
            "" => IconMode::Nerd,
            _ => IconMode::Nerd,
        }
    }

    pub fn is_nerd(self) -> bool {
        matches!(self, IconMode::Nerd)
    }

    pub fn is_ascii(self) -> bool {
        matches!(self, IconMode::Ascii)
    }
}

/// Install the icon mode used by every helper below.
pub fn set_icon_mode(mode: IconMode) {
    let v = match mode {
        IconMode::Nerd => MODE_NERD,
        IconMode::Ascii => MODE_ASCII,
    };
    ICON_MODE.store(v, Ordering::Relaxed);
}

/// Current icon emission mode.
pub fn icon_mode() -> IconMode {
    match ICON_MODE.load(Ordering::Relaxed) {
        MODE_ASCII => IconMode::Ascii,
        _ => IconMode::Nerd,
    }
}

/// Read `FORGE_ICONS` and install the mode. Call once at startup.
pub fn init_from_env() {
    let mode = match std::env::var("FORGE_ICONS") {
        Ok(v) => IconMode::parse(&v),
        Err(_) => IconMode::Nerd,
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

    /// Collapsed directory chevron (flat list — always collapsed for now).
    pub fn chevron_collapsed() -> &'static str {
        match icon_mode() {
            // nf-fa-chevron_right
            IconMode::Nerd => "\u{f054}",
            IconMode::Ascii => ">",
        }
    }

    pub fn chevron_expanded() -> &'static str {
        match icon_mode() {
            // nf-fa-chevron_down
            IconMode::Nerd => "\u{f078}",
            IconMode::Ascii => "v",
        }
    }

    pub fn folder(open: bool) -> &'static str {
        match (icon_mode(), open) {
            // nf-fa-folder_open / nf-fa-folder
            (IconMode::Nerd, true) => "\u{f07c}",
            (IconMode::Nerd, false) => "\u{f07b}",
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
        assert_eq!(IconMode::parse(""), IconMode::Nerd);
        assert_eq!(IconMode::parse("wat"), IconMode::Nerd);
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
        // restore default for other tests in the same process
        set_icon_mode(IconMode::Nerd);
    }

    #[test]
    fn padded_keeps_trailing_space() {
        set_icon_mode(IconMode::Ascii);
        assert_eq!(Icons::padded(Icons::folder_closed()), "/ ");
        assert_eq!(Icons::padded(""), "  ");
        set_icon_mode(IconMode::Nerd);
    }
}
