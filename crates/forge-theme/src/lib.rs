//! Theme tokens for the forge TUI.
//!
//! Default look is a **modern dark IDE** (charcoal + cyan), matching the
//! product mock — NOT Turbo Vision blue. Palette RGB values map to ANSI-256
//! indexed colors by default so charcoal greys remain visible without truecolor.

#![forbid(unsafe_code)]

/// 24-bit RGB color (palette token; UI maps via ColorMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Classic VGA / BIOS 16-color table (exact match targets for `match_vga`).
pub mod vga {
    use super::Rgb;

    pub const BLACK: Rgb = Rgb::new(0x00, 0x00, 0x00);
    pub const BLUE: Rgb = Rgb::new(0x00, 0x00, 0xAA);
    pub const GREEN: Rgb = Rgb::new(0x00, 0xAA, 0x00);
    pub const CYAN: Rgb = Rgb::new(0x00, 0xAA, 0xAA);
    pub const RED: Rgb = Rgb::new(0xAA, 0x00, 0x00);
    pub const MAGENTA: Rgb = Rgb::new(0xAA, 0x00, 0xAA);
    pub const BROWN: Rgb = Rgb::new(0xAA, 0x55, 0x00);
    pub const LIGHT_GRAY: Rgb = Rgb::new(0xAA, 0xAA, 0xAA);
    pub const DARK_GRAY: Rgb = Rgb::new(0x55, 0x55, 0x55);
    pub const LIGHT_BLUE: Rgb = Rgb::new(0x55, 0x55, 0xFF);
    pub const LIGHT_GREEN: Rgb = Rgb::new(0x55, 0xFF, 0x55);
    pub const LIGHT_CYAN: Rgb = Rgb::new(0x55, 0xFF, 0xFF);
    pub const LIGHT_RED: Rgb = Rgb::new(0xFF, 0x55, 0x55);
    pub const LIGHT_MAGENTA: Rgb = Rgb::new(0xFF, 0x55, 0xFF);
    pub const YELLOW: Rgb = Rgb::new(0xFF, 0xFF, 0x55);
    pub const WHITE: Rgb = Rgb::new(0xFF, 0xFF, 0xFF);

    pub const ALL: [Rgb; 16] = [
        BLACK, BLUE, GREEN, CYAN, RED, MAGENTA, BROWN, LIGHT_GRAY, DARK_GRAY, LIGHT_BLUE,
        LIGHT_GREEN, LIGHT_CYAN, LIGHT_RED, LIGHT_MAGENTA, YELLOW, WHITE,
    ];
}

/// Named VGA index (0..=15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VgaNamed {
    Black,
    Blue,
    Green,
    Cyan,
    Red,
    Magenta,
    Brown,
    LightGray,
    DarkGray,
    LightBlue,
    LightGreen,
    LightCyan,
    LightRed,
    LightMagenta,
    Yellow,
    White,
}

impl VgaNamed {
    pub const ALL: [VgaNamed; 16] = [
        VgaNamed::Black,
        VgaNamed::Blue,
        VgaNamed::Green,
        VgaNamed::Cyan,
        VgaNamed::Red,
        VgaNamed::Magenta,
        VgaNamed::Brown,
        VgaNamed::LightGray,
        VgaNamed::DarkGray,
        VgaNamed::LightBlue,
        VgaNamed::LightGreen,
        VgaNamed::LightCyan,
        VgaNamed::LightRed,
        VgaNamed::LightMagenta,
        VgaNamed::Yellow,
        VgaNamed::White,
    ];

    pub fn rgb(self) -> Rgb {
        vga::ALL[self as usize]
    }
}

pub fn match_vga(c: Rgb) -> Option<VgaNamed> {
    VgaNamed::ALL.into_iter().find(|&n| n.rgb() == c)
}

pub fn nearest_vga(c: Rgb) -> VgaNamed {
    if let Some(n) = match_vga(c) {
        return n;
    }
    let mut best = VgaNamed::Black;
    let mut best_d = u32::MAX;
    for n in VgaNamed::ALL {
        let v = n.rgb();
        let dr = c.r as i32 - v.r as i32;
        let dg = c.g as i32 - v.g as i32;
        let db = c.b as i32 - v.b as i32;
        let d = (dr * dr + dg * dg + db * db) as u32;
        if d < best_d {
            best_d = d;
            best = n;
        }
    }
    best
}

/// How forge emits colors to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Named ANSI 16 colors only.
    Ansi16,
    /// `\e[38;5;Nm` indexed — default for modern charcoal theme.
    #[default]
    Ansi256,
    /// `\e[38;2;r;g;bm` — needs truecolor.
    TrueColor,
}

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Ansi16 => "ansi16",
            ColorMode::Ansi256 => "ansi256",
            ColorMode::TrueColor => "truecolor",
        }
    }
}

/// Detect color mode. Defaults to **Ansi256** so charcoal greys render.
pub fn detect_color_mode() -> ColorMode {
    if let Ok(v) = std::env::var("FORGE_COLOR") {
        let v = v.to_ascii_lowercase();
        match v.as_str() {
            "16" | "ansi16" | "vga" | "basic" => return ColorMode::Ansi16,
            "256" | "ansi256" | "ansi" => return ColorMode::Ansi256,
            "truecolor" | "24bit" | "24" | "rgb" => return ColorMode::TrueColor,
            _ => {}
        }
    }

    if let Ok(v) = std::env::var("COLORTERM") {
        let v = v.to_ascii_lowercase();
        if v.contains("truecolor") || v.contains("24bit") {
            return ColorMode::TrueColor;
        }
    }

    ColorMode::Ansi256
}

/// Map an RGB triplet onto the xterm 256-color cube (16..231) + grayscale.
pub fn to_ansi256(c: Rgb) -> u8 {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max - min < 8 {
        let gray = (c.r as u16 + c.g as u16 + c.b as u16) / 3;
        if gray < 8 {
            return 16;
        }
        if gray > 248 {
            return 231;
        }
        return (232 + ((gray - 8) * 24) / 247) as u8;
    }

    let ri = ((c.r as u16 * 5 + 127) / 255) as u8;
    let gi = ((c.g as u16 * 5 + 127) / 255) as u8;
    let bi = ((c.b as u16 * 5 + 127) / 255) as u8;
    16 + 36 * ri + 6 * gi + bi
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorToken {
    Fg,
    Bg,
    Muted,
    Accent,
    Border,
    BorderFocus,
    SelectionBg,
    SelectionFg,
    GitModified,
    GitAdded,
    GitDeleted,
    GitUntracked,
    GitIgnored,
    GitConflicted,
    GitRenamed,
    DiffAdd,
    DiffAddBg,
    DiffDel,
    DiffDelBg,
    Directory,
    HeaderBg,
    PanelBg,
    Danger,
    Warning,
    ChromeFg,
    ChromeBg,
    ChromeHotkey,
    TitleActive,
    TitleInactive,
    MenuBg,
    MenuFg,
    Shadow,
    PillBg,
    BranchFg,
    BranchBg,
    BadgeFg,
    BadgeBg,
    ActivityBg,
    TabActiveFg,
    TabActiveBg,
    TabInactiveFg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub bg: Rgb,
    pub panel_bg: Rgb,
    pub header_bg: Rgb,
    pub fg: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    pub border: Rgb,
    pub border_focus: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
    pub git_modified: Rgb,
    pub git_added: Rgb,
    pub git_deleted: Rgb,
    pub git_untracked: Rgb,
    pub git_ignored: Rgb,
    pub git_conflicted: Rgb,
    pub git_renamed: Rgb,
    pub diff_add: Rgb,
    pub diff_add_bg: Rgb,
    pub diff_del: Rgb,
    pub diff_del_bg: Rgb,
    pub directory: Rgb,
    pub danger: Rgb,
    pub warning: Rgb,
    pub chrome_fg: Rgb,
    pub chrome_bg: Rgb,
    pub chrome_hotkey: Rgb,
    pub title_active: Rgb,
    pub title_inactive: Rgb,
    pub menu_bg: Rgb,
    pub menu_fg: Rgb,
    pub shadow: Rgb,
    pub pill_bg: Rgb,
    pub branch_fg: Rgb,
    pub branch_bg: Rgb,
    pub badge_fg: Rgb,
    pub badge_bg: Rgb,
    pub activity_bg: Rgb,
    pub tab_active_fg: Rgb,
    pub tab_active_bg: Rgb,
    pub tab_inactive_fg: Rgb,
}

impl Theme {
    pub fn color(&self, token: ColorToken) -> Rgb {
        match token {
            ColorToken::Fg => self.fg,
            ColorToken::Bg => self.bg,
            ColorToken::Muted => self.muted,
            ColorToken::Accent => self.accent,
            ColorToken::Border => self.border,
            ColorToken::BorderFocus => self.border_focus,
            ColorToken::SelectionBg => self.selection_bg,
            ColorToken::SelectionFg => self.selection_fg,
            ColorToken::GitModified => self.git_modified,
            ColorToken::GitAdded => self.git_added,
            ColorToken::GitDeleted => self.git_deleted,
            ColorToken::GitUntracked => self.git_untracked,
            ColorToken::GitIgnored => self.git_ignored,
            ColorToken::GitConflicted => self.git_conflicted,
            ColorToken::GitRenamed => self.git_renamed,
            ColorToken::DiffAdd => self.diff_add,
            ColorToken::DiffAddBg => self.diff_add_bg,
            ColorToken::DiffDel => self.diff_del,
            ColorToken::DiffDelBg => self.diff_del_bg,
            ColorToken::Directory => self.directory,
            ColorToken::HeaderBg => self.header_bg,
            ColorToken::PanelBg => self.panel_bg,
            ColorToken::Danger => self.danger,
            ColorToken::Warning => self.warning,
            ColorToken::ChromeFg => self.chrome_fg,
            ColorToken::ChromeBg => self.chrome_bg,
            ColorToken::ChromeHotkey => self.chrome_hotkey,
            ColorToken::TitleActive => self.title_active,
            ColorToken::TitleInactive => self.title_inactive,
            ColorToken::MenuBg => self.menu_bg,
            ColorToken::MenuFg => self.menu_fg,
            ColorToken::Shadow => self.shadow,
            ColorToken::PillBg => self.pill_bg,
            ColorToken::BranchFg => self.branch_fg,
            ColorToken::BranchBg => self.branch_bg,
            ColorToken::BadgeFg => self.badge_fg,
            ColorToken::BadgeBg => self.badge_bg,
            ColorToken::ActivityBg => self.activity_bg,
            ColorToken::TabActiveFg => self.tab_active_fg,
            ColorToken::TabActiveBg => self.tab_active_bg,
            ColorToken::TabInactiveFg => self.tab_inactive_fg,
        }
    }

    /// Modern dark IDE — charcoal + cyan (product mock).
    pub fn modern_ide() -> Self {
        Self {
            name: "modern-ide".into(),
            // Deep charcoal desktop / panels
            bg: Rgb::new(0x0d, 0x0f, 0x14),
            panel_bg: Rgb::new(0x12, 0x15, 0x1c),
            header_bg: Rgb::new(0x10, 0x13, 0x1a),
            fg: Rgb::new(0xe6, 0xea, 0xf2),
            muted: Rgb::new(0x7a, 0x84, 0x96),
            accent: Rgb::new(0x3d, 0xd6, 0xe0),
            border: Rgb::new(0x2a, 0x31, 0x3e),
            border_focus: Rgb::new(0x3d, 0xd6, 0xe0),
            // Edit-style solid cyan selection (hard invert)
            selection_bg: Rgb::new(0x3d, 0xd6, 0xe0),
            selection_fg: Rgb::new(0x0d, 0x0f, 0x14),
            git_modified: Rgb::new(0xf0, 0xb4, 0x4c),
            git_added: Rgb::new(0x5d, 0xd6, 0x8a),
            git_deleted: Rgb::new(0xf0, 0x6e, 0x6e),
            git_untracked: Rgb::new(0x7a, 0xb8, 0xf0),
            git_ignored: Rgb::new(0x4a, 0x52, 0x60),
            git_conflicted: Rgb::new(0xf0, 0x98, 0x5c),
            git_renamed: Rgb::new(0x5c, 0xc8, 0xd8),
            diff_add: Rgb::new(0xa0, 0xe4, 0xb8),
            diff_add_bg: Rgb::new(0x14, 0x2a, 0x1c),
            diff_del: Rgb::new(0xf0, 0xb0, 0xb0),
            diff_del_bg: Rgb::new(0x32, 0x16, 0x18),
            directory: Rgb::new(0x6c, 0xb6, 0xff),
            danger: Rgb::new(0xf0, 0x6e, 0x6e),
            warning: Rgb::new(0xf0, 0xb4, 0x4c),
            chrome_fg: Rgb::new(0xc8, 0xd0, 0xdc),
            chrome_bg: Rgb::new(0x0f, 0x12, 0x18),
            chrome_hotkey: Rgb::new(0x3d, 0xd6, 0xe0),
            title_active: Rgb::new(0xe6, 0xea, 0xf2),
            title_inactive: Rgb::new(0x7a, 0x84, 0x96),
            menu_bg: Rgb::new(0x1a, 0x1f, 0x2a),
            menu_fg: Rgb::new(0xe6, 0xea, 0xf2),
            shadow: Rgb::new(0x00, 0x00, 0x00),
            pill_bg: Rgb::new(0x1a, 0x1f, 0x28),
            branch_fg: Rgb::new(0x0a, 0x18, 0x10),
            branch_bg: Rgb::new(0x3d, 0xc4, 0x78),
            badge_fg: Rgb::new(0x20, 0x14, 0x08),
            badge_bg: Rgb::new(0xf0, 0x98, 0x3c),
            activity_bg: Rgb::new(0x0b, 0x0d, 0x12),
            tab_active_fg: Rgb::new(0xf0, 0xfb, 0xff),
            tab_active_bg: Rgb::new(0x12, 0x15, 0x1c),
            tab_inactive_fg: Rgb::new(0x7a, 0x84, 0x96),
        }
    }

    /// Ansi16-safe dark theme (named VGA only) — fallback when forced to 16 colors.
    pub fn modern_ide_vga() -> Self {
        use vga::*;
        Self {
            name: "modern-ide-vga".into(),
            bg: BLACK,
            panel_bg: BLACK,
            header_bg: BLACK,
            fg: WHITE,
            muted: DARK_GRAY,
            accent: LIGHT_CYAN,
            border: DARK_GRAY,
            border_focus: LIGHT_CYAN,
            selection_bg: CYAN,
            selection_fg: BLACK,
            git_modified: YELLOW,
            git_added: LIGHT_GREEN,
            git_deleted: LIGHT_RED,
            git_untracked: LIGHT_BLUE,
            git_ignored: DARK_GRAY,
            git_conflicted: LIGHT_MAGENTA,
            git_renamed: CYAN,
            diff_add: LIGHT_GREEN,
            diff_add_bg: GREEN,
            diff_del: LIGHT_RED,
            diff_del_bg: RED,
            directory: LIGHT_BLUE,
            danger: LIGHT_RED,
            warning: YELLOW,
            chrome_fg: LIGHT_GRAY,
            chrome_bg: BLACK,
            chrome_hotkey: LIGHT_CYAN,
            title_active: WHITE,
            title_inactive: DARK_GRAY,
            menu_bg: DARK_GRAY,
            menu_fg: WHITE,
            shadow: BLACK,
            pill_bg: DARK_GRAY,
            branch_fg: BLACK,
            branch_bg: LIGHT_GREEN,
            badge_fg: BLACK,
            badge_bg: BROWN,
            activity_bg: BLACK,
            tab_active_fg: BLACK,
            tab_active_bg: LIGHT_CYAN,
            tab_inactive_fg: DARK_GRAY,
        }
    }

    /// Edit terminal-film aesthetic (legacy).
    pub fn edit_terminal() -> Self {
        use vga::*;
        Self {
            name: "edit-terminal".into(),
            bg: BLUE,
            panel_bg: BLACK,
            header_bg: BLUE,
            fg: WHITE,
            muted: DARK_GRAY,
            accent: LIGHT_CYAN,
            border: WHITE,
            border_focus: LIGHT_CYAN,
            selection_bg: LIGHT_CYAN,
            selection_fg: BLACK,
            git_modified: YELLOW,
            git_added: LIGHT_GREEN,
            git_deleted: LIGHT_RED,
            git_untracked: LIGHT_CYAN,
            git_ignored: DARK_GRAY,
            git_conflicted: LIGHT_MAGENTA,
            git_renamed: CYAN,
            diff_add: LIGHT_GREEN,
            diff_add_bg: GREEN,
            diff_del: LIGHT_RED,
            diff_del_bg: RED,
            directory: LIGHT_BLUE,
            danger: LIGHT_RED,
            warning: YELLOW,
            chrome_fg: BLACK,
            chrome_bg: LIGHT_GRAY,
            chrome_hotkey: RED,
            title_active: WHITE,
            title_inactive: LIGHT_GRAY,
            menu_bg: DARK_GRAY,
            menu_fg: WHITE,
            shadow: BLACK,
            pill_bg: DARK_GRAY,
            branch_fg: BLACK,
            branch_bg: LIGHT_GREEN,
            badge_fg: BLACK,
            badge_bg: BROWN,
            activity_bg: BLUE,
            tab_active_fg: BLACK,
            tab_active_bg: LIGHT_CYAN,
            tab_inactive_fg: LIGHT_GRAY,
        }
    }

    pub fn turbo_vision() -> Self {
        use vga::*;
        Self {
            name: "turbo-vision".into(),
            bg: BLUE,
            panel_bg: BLUE,
            header_bg: LIGHT_GRAY,
            fg: WHITE,
            muted: LIGHT_CYAN,
            accent: YELLOW,
            border: WHITE,
            border_focus: YELLOW,
            selection_bg: GREEN,
            selection_fg: WHITE,
            git_modified: YELLOW,
            git_added: LIGHT_GREEN,
            git_deleted: LIGHT_RED,
            git_untracked: LIGHT_CYAN,
            git_ignored: DARK_GRAY,
            git_conflicted: LIGHT_MAGENTA,
            git_renamed: CYAN,
            diff_add: LIGHT_GREEN,
            diff_add_bg: GREEN,
            diff_del: LIGHT_RED,
            diff_del_bg: RED,
            directory: LIGHT_CYAN,
            danger: LIGHT_RED,
            warning: YELLOW,
            chrome_fg: BLACK,
            chrome_bg: LIGHT_GRAY,
            chrome_hotkey: RED,
            title_active: YELLOW,
            title_inactive: WHITE,
            menu_bg: LIGHT_GRAY,
            menu_fg: BLACK,
            shadow: BLACK,
            pill_bg: LIGHT_GRAY,
            branch_fg: BLACK,
            branch_bg: LIGHT_GREEN,
            badge_fg: BLACK,
            badge_bg: BROWN,
            activity_bg: BLUE,
            tab_active_fg: BLACK,
            tab_active_bg: YELLOW,
            tab_inactive_fg: WHITE,
        }
    }

    pub fn grok_dark() -> Self {
        Self::modern_ide()
    }

    pub fn default_dark() -> Self {
        Self::modern_ide()
    }

    pub fn assert_vga_only(&self) {
        let slots = [
            self.bg, self.panel_bg, self.header_bg, self.fg, self.muted, self.accent,
            self.border, self.border_focus, self.selection_bg, self.selection_fg,
            self.git_modified, self.git_added, self.git_deleted, self.git_untracked,
            self.git_ignored, self.git_conflicted, self.git_renamed, self.diff_add,
            self.diff_add_bg, self.diff_del, self.diff_del_bg, self.directory,
            self.danger, self.warning, self.chrome_fg, self.chrome_bg, self.chrome_hotkey,
            self.title_active, self.title_inactive, self.menu_bg, self.menu_fg, self.shadow,
            self.pill_bg, self.branch_fg, self.branch_bg, self.badge_fg, self.badge_bg,
            self.activity_bg, self.tab_active_fg, self.tab_active_bg, self.tab_inactive_fg,
        ];
        for c in slots {
            assert!(
                match_vga(c).is_some(),
                "non-VGA color in theme '{}': #{:02x}{:02x}{:02x}",
                self.name,
                c.r,
                c.g,
                c.b
            );
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::modern_ide()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_themes_are_vga_only() {
        Theme::edit_terminal().assert_vga_only();
        Theme::turbo_vision().assert_vga_only();
        Theme::modern_ide_vga().assert_vga_only();
    }

    #[test]
    fn nearest_maps_charcoal_to_black() {
        assert_eq!(nearest_vga(Rgb::new(0x10, 0x10, 0x14)), VgaNamed::Black);
        assert_eq!(nearest_vga(vga::BLUE), VgaNamed::Blue);
    }

    #[test]
    fn detect_defaults_ansi256() {
        std::env::remove_var("FORGE_COLOR");
        std::env::remove_var("COLORTERM");
        assert_eq!(detect_color_mode(), ColorMode::Ansi256);
    }

    #[test]
    fn detect_respects_forge_color_env() {
        std::env::set_var("FORGE_COLOR", "truecolor");
        assert_eq!(detect_color_mode(), ColorMode::TrueColor);
        std::env::set_var("FORGE_COLOR", "256");
        assert_eq!(detect_color_mode(), ColorMode::Ansi256);
        std::env::set_var("FORGE_COLOR", "16");
        assert_eq!(detect_color_mode(), ColorMode::Ansi16);
        std::env::remove_var("FORGE_COLOR");
    }

    #[test]
    fn color_mode_labels() {
        assert_eq!(ColorMode::Ansi16.as_str(), "ansi16");
        assert_eq!(ColorMode::Ansi256.as_str(), "ansi256");
        assert_eq!(ColorMode::TrueColor.as_str(), "truecolor");
    }

    #[test]
    fn modern_ide_has_cyan_accent() {
        let t = Theme::modern_ide();
        assert_eq!(t.name, "modern-ide");
        assert!(t.accent.g > 180);
        assert!(t.bg.r < 30);
    }

    #[test]
    fn modern_ide_selection_is_hard_invert_cyan() {
        let t = Theme::modern_ide();
        // Bright cyan/accent-like bg, near-black fg (Edit signature)
        assert!(t.selection_bg.g > 180);
        assert!(t.selection_bg.b > 180);
        assert!(t.selection_fg.r < 40);
        assert!(t.selection_fg.g < 40);
        assert!(t.selection_fg.b < 40);
    }
}
