//! Platform-independent shell profiles; all profiles share the native PTY.
use forge_config::TerminalConfig;
use portable_pty::CommandBuilder;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[cfg(not(windows))]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone)]
pub(crate) struct ShellProfile {
    pub id: String,
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    #[cfg(windows)]
    pub wsl: bool,
}
impl ShellProfile {
    fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        command: PathBuf,
        args: &[&str],
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command,
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            env: BTreeMap::new(),
            cwd: None,
            #[cfg(windows)]
            wsl: false,
        }
    }
    pub fn builder(&self, workspace: &Path) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(&self.command);
        cmd.args(&self.args);
        let cwd = self.cwd.as_deref().unwrap_or(workspace);
        #[cfg(windows)]
        if self.wsl {
            cmd.arg("--cd");
            cmd.arg(cwd);
        }
        cmd.cwd(cwd);
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}
fn find_exe(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return executable(path).then(|| path.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .map(|p| p.join(name))
        .find(|p| executable(p))
}
fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
pub(crate) fn local_profiles(config: &TerminalConfig) -> Vec<ShellProfile> {
    #[cfg(windows)]
    let mut profiles = windows::discover_local();
    #[cfg(not(windows))]
    let mut profiles = unix::discover();
    for (id, p) in &config.profiles {
        if p.command.as_os_str().is_empty() {
            continue;
        }
        let command = if p.command.is_absolute() && executable(&p.command) {
            Some(p.command.clone())
        } else {
            find_exe(&p.command.to_string_lossy())
        };
        let Some(command) = command else {
            continue;
        };
        let profile = ShellProfile {
            id: id.clone(),
            name: p.name.clone().unwrap_or_else(|| id.clone()),
            command,
            args: p.args.clone(),
            env: p.env.clone(),
            cwd: p.cwd.clone(),
            #[cfg(windows)]
            wsl: false,
        };
        if let Some(existing) = profiles.iter_mut().find(|existing| existing.id == *id) {
            *existing = profile;
        } else {
            profiles.push(profile);
        }
    }
    profiles
}
pub(crate) fn discover(config: &TerminalConfig) -> Vec<ShellProfile> {
    let profiles = local_profiles(config);
    #[cfg(windows)]
    {
        let mut profiles = profiles;
        for profile in windows::discover_wsl() {
            if !profiles.iter().any(|p| p.id == profile.id) {
                profiles.push(profile);
            }
        }
        profiles
    }
    #[cfg(not(windows))]
    {
        profiles
    }
}
pub(crate) fn resolve_default(
    config: &TerminalConfig,
    profiles: &[ShellProfile],
) -> anyhow::Result<Option<ShellProfile>> {
    if let Some(id) = &config.default_profile {
        if let Some(profile) = profiles.iter().find(|p| &p.id == id) {
            return Ok(Some(profile.clone()));
        }
        #[cfg(windows)]
        if let Some(distro) = id.strip_prefix("wsl:") {
            if let Some(p) = windows::wsl_profile(distro) {
                return Ok(Some(p));
            }
        }
        anyhow::bail!("Default shell profile '{id}' is unavailable; select another profile");
    }
    Ok(profiles.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_arguments_and_custom_defaults() {
        let config = TerminalConfig::default();
        let p = ShellProfile::new("a", "A", PathBuf::from("shell"), &["-i"]);
        assert_eq!(
            resolve_default(&config, &[p.clone()]).unwrap().unwrap().id,
            "a"
        );
        let config = TerminalConfig {
            default_profile: Some("missing".into()),
            ..config
        };
        assert!(resolve_default(&config, &[p]).is_err());
    }
    #[test]
    fn discovered_local_profiles_have_unique_ids_and_executables() {
        let profiles = local_profiles(&TerminalConfig::default());
        assert!(!profiles.is_empty());
        let ids: std::collections::HashSet<_> = profiles.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), profiles.len());
        assert!(profiles.iter().all(|p| executable(&p.command)));
    }
}
