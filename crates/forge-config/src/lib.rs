//! User configuration for Forge, separate from keyboard overrides.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

fn default_sticky() -> bool {
    true
}

fn default_compact() -> bool {
    true
}

/// Missing fields are compatible with older Forge configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub git: GitConfig,
    pub theme: Option<String>,
    /// Pin parent directories above scrolled Files / Git / History trees.
    #[serde(default = "default_sticky")]
    pub tree_sticky_scroll: bool,
    /// Merge expanded, single-child folder paths in Files and Git trees.
    #[serde(default = "default_compact")]
    pub compact_folders: bool,
    /// Include dotfiles in the Files tree (Git tracked changes remain visible).
    pub show_hidden_files: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            git: GitConfig::default(),
            theme: None,
            tree_sticky_scroll: true,
            compact_folders: true,
            show_hidden_files: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    pub blame: BlameConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BlameConfig {
    pub enabled: bool,
    pub show_inline: bool,
    pub show_popup: bool,
    pub show_uncommitted: bool,
}

impl Default for BlameConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_inline: true,
            show_popup: true,
            show_uncommitted: true,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("forge/config.toml"))
    }

    pub fn load_checked() -> Result<Self, String> {
        let path = Self::path().ok_or("HOME/XDG_CONFIG_HOME is not set")?;
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn load_default() -> Self {
        Self::load_checked().unwrap_or_default()
    }

    /// Preserve unrelated settings and fail visibly instead of reporting a
    /// successful toggle that would be lost after restart.
    pub fn save_sticky_scroll(&self) -> Result<(), String> {
        self.save()
    }

    /// Preserve unrelated TOML keys when changing preferences.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("HOME/XDG_CONFIG_HOME is not set")?;
        let mut table: toml::Table = match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content)
                .map_err(|e| format!("Cannot update invalid config: {e}"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(e.to_string()),
        };
        table.insert(
            "tree_sticky_scroll".into(),
            toml::Value::Boolean(self.tree_sticky_scroll),
        );
        table.insert(
            "compact_folders".into(),
            toml::Value::Boolean(self.compact_folders),
        );
        table.insert(
            "show_hidden_files".into(),
            toml::Value::Boolean(self.show_hidden_files),
        );
        let text = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
        let parent = path.parent().ok_or("Invalid configuration path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_old_configuration() {
        assert!(Config::default().tree_sticky_scroll);
        let old: Config = toml::from_str("theme = 'classic'").unwrap();
        assert!(old.tree_sticky_scroll);
        assert_eq!(old.theme.as_deref(), Some("classic"));
        let off: Config = toml::from_str("tree_sticky_scroll = false").unwrap();
        assert!(!off.tree_sticky_scroll);
        assert!(!off.show_hidden_files);
        assert!(
            off.compact_folders,
            "old configurations default to compact folders"
        );
        let verbose: Config = toml::from_str("compact_folders = false").unwrap();
        assert!(!verbose.compact_folders);
        let shown: Config = toml::from_str("show_hidden_files = true").unwrap();
        assert!(shown.show_hidden_files);
    }

    #[test]
    fn nested_blame_preferences_default_independently() {
        let old: Config = toml::from_str("").unwrap();
        assert!(old.git.blame.enabled && old.git.blame.show_inline);
        assert!(old.git.blame.show_popup && old.git.blame.show_uncommitted);
        let custom: Config =
            toml::from_str("[git.blame]\nshow_inline = false\nshow_uncommitted = false").unwrap();
        assert!(custom.git.blame.enabled && custom.git.blame.show_popup);
        assert!(!custom.git.blame.show_inline && !custom.git.blame.show_uncommitted);
        let disabled: Config =
            toml::from_str("[git.blame]\nenabled = false\nshow_popup = false").unwrap();
        assert!(!disabled.git.blame.enabled && !disabled.git.blame.show_popup);
    }
}
