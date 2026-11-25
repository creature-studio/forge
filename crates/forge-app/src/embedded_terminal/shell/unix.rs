use super::*;
pub(super) fn discover() -> Vec<ShellProfile> {
    let mut out = Vec::new();
    let mut add = |path: PathBuf| {
        if !executable(&path) {
            return;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let id = path.to_string_lossy().into_owned();
        if out.iter().any(|p: &ShellProfile| {
            p.id == id
                || (p.name == name && p.command.canonicalize().ok() == path.canonicalize().ok())
        }) {
            return;
        }
        out.push(ShellProfile::new(id, name, path, &[]));
    };
    // Preserve the login shell even if SHELL is absent or invalid.
    let login = CommandBuilder::new_default_prog().get_shell();
    if let Some(path) = find_exe(&login) {
        add(path);
    }
    if let Ok(content) = std::fs::read_to_string("/etc/shells") {
        for line in content
            .lines()
            .map(str::trim)
            .filter(|s| s.starts_with('/'))
        {
            add(PathBuf::from(line));
        }
    }
    for name in ["bash", "zsh", "fish", "sh", "dash", "ksh", "nu", "pwsh"] {
        if let Some(path) = find_exe(name) {
            add(path);
        }
        #[cfg(target_os = "macos")]
        for root in ["/opt/homebrew/bin", "/usr/local/bin"] {
            add(Path::new(root).join(name));
        }
    }
    out
}
