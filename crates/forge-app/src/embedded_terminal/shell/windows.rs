use super::*;
use std::{os::windows::process::CommandExt, process::Command};
const CREATE_NO_WINDOW: u32 = 0x08000000;
fn system_path(relative: &str) -> Option<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|p| p.join(relative))
        .filter(|p| executable(p))
}
// Query both registry views for installers outside the standard directories.
fn registry_paths(key: &str, value: &str) -> Vec<PathBuf> {
    use winreg::{
        enums::{
            HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        },
        RegKey,
    };
    let mut paths = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            if let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(key, KEY_READ | view) {
                if let Ok(path) = key.get_value::<String, _>(value) {
                    let path = PathBuf::from(path);
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    paths
}
fn locate(name: &str, paths: Vec<PathBuf>) -> Option<PathBuf> {
    find_exe(name).or_else(|| paths.into_iter().find(|p| executable(p)))
}
pub(super) fn discover_local() -> Vec<ShellProfile> {
    let mut out = Vec::new();
    let roots: Vec<_> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    let powershell_paths: Vec<_> = roots
        .iter()
        .flat_map(|r| {
            [
                r.join("PowerShell/7/pwsh.exe"),
                r.join("Programs/PowerShell/7/pwsh.exe"),
            ]
        })
        .chain(registry_paths(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\pwsh.exe",
            "",
        ))
        .collect();
    if let Some(p) = locate("pwsh.exe", powershell_paths) {
        out.push(ShellProfile::new(
            "powershell7",
            "PowerShell 7",
            p,
            &["-NoLogo"],
        ));
    }
    if let Some(p) = find_exe("powershell.exe")
        .or_else(|| system_path("System32/WindowsPowerShell/v1.0/powershell.exe"))
    {
        out.push(ShellProfile::new(
            "powershell",
            "Windows PowerShell",
            p,
            &["-NoLogo"],
        ));
    }
    if let Some(p) = std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .filter(|p| executable(p))
        .or_else(|| system_path("System32/cmd.exe"))
        .or_else(|| find_exe("cmd.exe"))
    {
        out.push(ShellProfile::new("cmd", "Command Prompt", p, &["/D", "/K"]));
    }
    let mut git_paths: Vec<_> = roots
        .iter()
        .flat_map(|r| {
            [
                r.join("Git/bin/bash.exe"),
                r.join("Programs/Git/bin/bash.exe"),
            ]
        })
        .collect();
    git_paths.extend(
        registry_paths(r"SOFTWARE\GitForWindows", "InstallPath")
            .into_iter()
            .map(|p| p.join("bin/bash.exe")),
    );
    if let Some(git) = find_exe("git.exe") {
        if let Some(root) = git.parent().and_then(Path::parent) {
            git_paths.push(root.join("bin/bash.exe"));
        }
    }
    if let Some(p) = git_paths.into_iter().find(|p| executable(p)) {
        out.push(ShellProfile::new(
            "git-bash",
            "Git Bash",
            p,
            &["--login", "-i"],
        ));
    }
    let cygwin = registry_paths(r"SOFTWARE\Cygwin\setup", "rootdir")
        .into_iter()
        .map(|p| p.join("bin/bash.exe"))
        .find(|p| executable(p));
    for (id, name, path) in [
        ("msys2", "MSYS2 Bash", r"C:\msys64\usr\bin\bash.exe"),
        ("cygwin", "Cygwin Bash", r"C:\cygwin64\bin\bash.exe"),
    ] {
        let p = if id == "cygwin" {
            cygwin.clone().unwrap_or_else(|| PathBuf::from(path))
        } else {
            PathBuf::from(path)
        };
        if executable(&p) {
            out.push(ShellProfile::new(id, name, p, &["--login", "-i"]));
        }
    }
    out
}
fn wsl_exe() -> Option<PathBuf> {
    system_path("System32/wsl.exe").or_else(|| find_exe("wsl.exe"))
}
pub(super) fn wsl_profile(distro: &str) -> Option<ShellProfile> {
    if distro.is_empty() || distro.chars().any(char::is_control) {
        return None;
    }
    let mut p = ShellProfile::new(
        format!("wsl:{distro}"),
        format!("WSL: {distro}"),
        wsl_exe()?,
        &["-d", distro],
    );
    p.wsl = true;
    Some(p)
}
pub(super) fn discover_wsl() -> Vec<ShellProfile> {
    let Some(exe) = wsl_exe() else {
        return Vec::new();
    };
    let Ok(out) = Command::new(exe)
        .args(["--list", "--quiet"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let bytes = out.stdout;
    let names = if bytes.starts_with(&[0xff, 0xfe]) || bytes.contains(&0) {
        String::from_utf16_lossy(
            &bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    names
        .lines()
        .map(|s| s.trim_matches(|c: char| c == '\u{feff}' || c.is_whitespace()))
        .filter_map(wsl_profile)
        .collect()
}
