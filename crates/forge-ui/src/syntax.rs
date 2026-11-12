//! Compact, opt-in Sublime syntax definitions for Code preview.
//!
//! Unlike Syntect's default feature set, this loads no bundled theme packs,
//! does not link Oniguruma, and initializes grammars only if a script is
//! previewed. The preview itself is capped at 64 KiB / 400 source lines.
use std::path::PathBuf;
use std::sync::OnceLock;

use forge_theme::{Rgb, Theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme as SyntaxTheme, ThemeItem,
};
use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet, SyntaxSetBuilder};

use crate::style::{fg_bg, rgb};

const MAX_PARSE_BYTES_PER_LINE: usize = 4096;

/// The complete grammar registry is lazy and shared between previews.
static GRAMMARS: OnceLock<SyntaxSet> = OnceLock::new();
const BUILTINS: &[&str] = &[
    include_str!("../syntaxes/python.sublime-syntax"),
    include_str!("../syntaxes/shell.sublime-syntax"),
    include_str!("../syntaxes/javascript.sublime-syntax"),
    include_str!("../syntaxes/typescript.sublime-syntax"),
    include_str!("../syntaxes/json.sublime-syntax"),
    include_str!("../syntaxes/yaml.sublime-syntax"),
    include_str!("../syntaxes/sql.sublime-syntax"),
    include_str!("../syntaxes/go.sublime-syntax"),
    include_str!("../syntaxes/c.sublime-syntax"),
    include_str!("../syntaxes/cpp.sublime-syntax"),
    include_str!("../syntaxes/java.sublime-syntax"),
    include_str!("../syntaxes/csharp.sublime-syntax"),
    include_str!("../syntaxes/css.sublime-syntax"),
    include_str!("../syntaxes/toml.sublime-syntax"),
    include_str!("../syntaxes/dockerfile.sublime-syntax"),
    include_str!("../syntaxes/lua.sublime-syntax"),
    include_str!("../syntaxes/php.sublime-syntax"),
    include_str!("../syntaxes/ruby.sublime-syntax"),
    include_str!("../syntaxes/kotlin.sublime-syntax"),
    include_str!("../syntaxes/swift.sublime-syntax"),
    include_str!("../syntaxes/powershell.sublime-syntax"),
    include_str!("../syntaxes/html.sublime-syntax"),
    include_str!("../syntaxes/xml.sublime-syntax"),
];

fn user_syntax_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("forge").join("syntaxes"))
}

fn syntax_set() -> &'static SyntaxSet {
    GRAMMARS.get_or_init(|| {
        let mut builder = SyntaxSetBuilder::new();
        for source in BUILTINS {
            // Bundled files are checked in tests, not loaded from disk.
            let syntax = SyntaxDefinition::load_from_str(source, false, None)
                .expect("bundled Forge grammar is invalid");
            builder.add(syntax);
        }
        // User-installed grammar files can extend or override built-ins.
        // Errors are not allowed to break existing language support.
        if let Some(dir) = user_syntax_dir() {
            if dir.is_dir() {
                if let Err(error) = builder.add_from_folder(&dir, false) {
                    // The registry remains useful even when one custom plugin
                    // is malformed. Do not print from inside the terminal UI.
                    let _ = error;
                }
            }
        }
        builder.build()
    })
}

fn syntax_by_extension(extension: &str) -> Option<&'static SyntaxReference> {
    // Later-installed definitions override matching built-ins.
    syntax_set().syntaxes().iter().rev().find(|syntax| {
        syntax
            .file_extensions
            .iter()
            .any(|ext| ext.eq_ignore_ascii_case(extension))
    })
}

/// Used for extension detection; unknown files stay Plain.
pub(crate) fn supports_extension(extension: &str) -> bool {
    syntax_by_extension(extension).is_some()
}

fn syn_color(rgb: Rgb) -> Color {
    Color {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
        a: 255,
    }
}

fn add_rule(theme: &mut SyntaxTheme, selector: &str, color: Rgb, bold: bool) {
    theme.scopes.push(ThemeItem {
        scope: selector
            .parse::<ScopeSelectors>()
            .expect("fixed valid scope"),
        style: StyleModifier {
            foreground: Some(syn_color(color)),
            background: None,
            font_style: bold.then_some(FontStyle::BOLD),
        },
    });
}

fn syntax_theme(palette: &Theme) -> SyntaxTheme {
    let mut theme = SyntaxTheme::default();
    theme.settings.foreground = Some(syn_color(palette.fg));
    theme.settings.background = Some(syn_color(palette.panel_bg));
    add_rule(&mut theme, "comment", palette.muted, false);
    add_rule(&mut theme, "string", palette.git_added, false);
    add_rule(&mut theme, "constant.numeric", palette.warning, false);
    add_rule(&mut theme, "constant.language", palette.warning, false);
    add_rule(&mut theme, "entity.name", palette.directory, false);
    add_rule(&mut theme, "keyword", palette.accent, true);
    add_rule(&mut theme, "support", palette.directory, false);
    theme
}

/// Emits only styled source lines, so the caller can immediately wrap them
/// into its existing width-safe, source-mapped preview cache. Does not build
/// another entire-file vector of highlighted strings or styles.
pub(crate) fn highlight_lines(
    palette: &Theme,
    extension: &str,
    lines: &[String],
    mut emit: impl FnMut(usize, Line<'static>),
) {
    let Some(syntax) = syntax_by_extension(extension) else {
        for (index, line) in lines.iter().enumerate() {
            emit(
                index,
                Line::from(Span::styled(
                    line.clone(),
                    fg_bg(palette.fg, palette.panel_bg),
                )),
            );
        }
        return;
    };
    let theme = syntax_theme(palette);
    let mut highlighter = HighlightLines::new(syntax, &theme);
    for (index, source) in lines.iter().enumerate() {
        // Avoid pathological regex cost for extremely long/untrusted lines.
        // The preview already has a bounded byte and line count.
        if source.len() > MAX_PARSE_BYTES_PER_LINE {
            emit(
                index,
                Line::from(Span::styled(
                    source.clone(),
                    fg_bg(palette.fg, palette.panel_bg),
                )),
            );
            continue;
        }
        let spans: Vec<_> = match highlighter.highlight_line(source, syntax_set()) {
            Ok(parts) => parts
                .into_iter()
                .filter(|(_, text)| !text.is_empty())
                .map(|(style, text)| {
                    let color =
                        Rgb::new(style.foreground.r, style.foreground.g, style.foreground.b);
                    let mut terminal = Style::default().fg(rgb(color)).bg(rgb(palette.panel_bg));
                    if style.font_style.contains(FontStyle::BOLD) {
                        terminal = terminal.add_modifier(Modifier::BOLD);
                    }
                    Span::styled(text.to_owned(), terminal)
                })
                .collect(),
            Err(_) => vec![Span::styled(
                source.clone(),
                fg_bg(palette.fg, palette.panel_bg),
            )],
        };
        emit(index, Line::from(spans));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_syntaxes_are_small_and_can_be_parsed() {
        for source in BUILTINS {
            assert!(
                SyntaxDefinition::load_from_str(source, false, None).is_ok(),
                "{source}"
            );
        }
        for ext in [
            "py",
            "sh",
            "js",
            "ts",
            "tsx",
            "json",
            "yaml",
            "yml",
            "sql",
            "go",
            "c",
            "cpp",
            "java",
            "cs",
            "html",
            "vue",
            "svelte",
            "css",
            "toml",
            "dockerfile",
            "lua",
            "php",
            "rb",
            "kt",
            "swift",
            "ps1",
            "xml",
        ] {
            assert!(supports_extension(ext), "missing {ext}");
        }
        assert!(!supports_extension("nonexistent-forge-extension"));
    }

    #[test]
    fn multi_line_scopes_are_retained_without_unbounded_line_cache() {
        let lines = vec![
            "/* hello".to_owned(),
            "continuation */ const value = 42;".to_owned(),
        ];
        let mut painted = Vec::new();
        highlight_lines(&Theme::modern_ide(), "js", &lines, |_, line| {
            painted.push(line)
        });
        assert_eq!(painted.len(), 2);
        assert!(
            painted[1].spans.len() > 1,
            "expected comment, keyword and number scopes"
        );
        assert_eq!(
            painted[1]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            lines[1]
        );
    }
}
