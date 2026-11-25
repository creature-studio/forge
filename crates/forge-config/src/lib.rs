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
    pub terminal: TerminalConfig,
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
    /// Soft-wrap code previews; enabled for existing installations by default.
    pub wrap_code: bool,
    /// Show compact Git/search navigation marks beside the preview.
    #[serde(alias = "show_minimap")]
    pub show_overview_ruler: bool,
    /// Last used workspace roots, most recent first (up to 12).
    pub recent_workspaces: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: TerminalConfig::default(),
            git: GitConfig::default(),
            theme: None,
            tree_sticky_scroll: true,
            compact_folders: true,
            show_hidden_files: false,
            wrap_code: true,
            show_overview_ruler: true,
            recent_workspaces: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub default_profile: Option<String>,
    pub profiles: std::collections::BTreeMap<String, ShellProfileConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellProfileConfig {
    pub name: Option<String>,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    pub blame: BlameConfig,
    pub show_unstaged_gutter: bool,
    pub show_staged_gutter: bool,
}
impl Default for GitConfig {
    fn default() -> Self {
        Self {
            blame: BlameConfig::default(),
            show_unstaged_gutter: true,
            show_staged_gutter: true,
        }
    }
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
            .or_else(|| {
                #[cfg(windows)]
                {
                    std::env::var_os("APPDATA").map(PathBuf::from)
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
            .map(|p| p.join("forge/config.toml"))
    }

    pub fn parse(content: &str) -> Result<Self, String> {
        toml::from_str(content).map_err(|e| e.to_string())
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
        table.insert("wrap_code".into(), toml::Value::Boolean(self.wrap_code));
        table.remove("show_minimap");
        table.insert(
            "show_overview_ruler".into(),
            toml::Value::Boolean(self.show_overview_ruler),
        );
        table.insert(
            "recent_workspaces".into(),
            toml::Value::Array(
                self.recent_workspaces
                    .iter()
                    .map(|path| toml::Value::String(path.to_string_lossy().into_owned()))
                    .collect(),
            ),
        );
        // Update only the explicitly managed keys. Preserve [git.blame] and
        // all unknown plugin/custom preferences in the same configuration.
        let git = table
            .entry("git")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let git = git
            .as_table_mut()
            .ok_or("Cannot save config: [git] must be a table")?;
        git.insert(
            "show_staged_gutter".into(),
            toml::Value::Boolean(self.git.show_staged_gutter),
        );
        git.insert(
            "show_unstaged_gutter".into(),
            toml::Value::Boolean(self.git.show_unstaged_gutter),
        );
        if let Some(theme) = &self.theme {
            table.insert("theme".into(), toml::Value::String(theme.clone()));
        } else {
            table.remove("theme");
        }
        let terminal = table
            .entry("terminal")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or("Cannot save config: [terminal] must be a table")?;
        if let Some(profile) = &self.terminal.default_profile {
            terminal.insert(
                "default_profile".into(),
                toml::Value::String(profile.clone()),
            );
        } else {
            terminal.remove("default_profile");
        }
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
    fn terminal_profiles_roundtrip_and_legacy_config_defaults() {
        let legacy: Config = toml::from_str("wrap_code = true").unwrap();
        assert!(legacy.terminal.default_profile.is_none());
        assert!(legacy.terminal.profiles.is_empty());
        let config: Config = toml::from_str(
            r#"
[terminal]
default_profile = "work"
[terminal.profiles.work]
name = "Work Bash"
command = "/bin/bash"
args = ["-i"]
cwd = "/tmp"
[terminal.profiles.work.env]
FORGE_TEST = "yes"
"#,
        )
        .unwrap();
        let p = &config.terminal.profiles["work"];
        assert_eq!(p.args, ["-i"]);
        assert_eq!(p.env["FORGE_TEST"], "yes");
        assert_eq!(p.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        let again: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(again.terminal.default_profile.as_deref(), Some("work"));
        assert_eq!(
            again.terminal.profiles["work"].command,
            PathBuf::from("/bin/bash")
        );
    }
    #[test]
    fn focus_ruler_defaults_on_and_can_be_persistently_disabled() {
        let old: Config = toml::from_str("wrap_code = true").unwrap();
        assert!(old.show_overview_ruler);
        let off: Config = toml::from_str("show_overview_ruler = false").unwrap();
        assert!(!off.show_overview_ruler);
        let encoded = toml::to_string(&off).unwrap();
        assert!(
            !toml::from_str::<Config>(&encoded)
                .unwrap()
                .show_overview_ruler
        );
    }

    #[test]
    fn code_wrap_is_enabled_for_old_files_and_persists_disabled() {
        let old: Config = toml::from_str("theme = 'current'").unwrap();
        assert!(old.wrap_code);
        let disabled: Config = toml::from_str("wrap_code = false").unwrap();
        assert!(!disabled.wrap_code);
        let encoded = toml::to_string(&disabled).unwrap();
        assert!(!toml::from_str::<Config>(&encoded).unwrap().wrap_code);
    }

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
    fn dual_git_gutter_layers_default_on_and_can_be_disabled_independently() {
        let default: Config = toml::from_str("").unwrap();
        assert!(default.git.show_staged_gutter && default.git.show_unstaged_gutter);
        let staged_off: Config = toml::from_str("[git]\nshow_staged_gutter = false").unwrap();
        assert!(!staged_off.git.show_staged_gutter && staged_off.git.show_unstaged_gutter);
        let unstaged_off: Config = toml::from_str("[git]\nshow_unstaged_gutter = false").unwrap();
        assert!(unstaged_off.git.show_staged_gutter && !unstaged_off.git.show_unstaged_gutter);
        let encoded = toml::to_string(&unstaged_off).unwrap();
        let restored: Config = toml::from_str(&encoded).unwrap();
        assert!(restored.git.show_staged_gutter && !restored.git.show_unstaged_gutter);
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
