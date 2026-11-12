use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

const REPOSITORY: &str = "creature-studio/forge";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version(u64, u64, u64);

/// Update Forge in place from the latest GitHub release.
pub fn run(check: bool, force: bool) -> Result<()> {
    if !cfg!(unix) {
        bail!("forge update is currently supported on Linux and macOS only");
    }
    if cfg!(target_env = "musl") {
        bail!("prebuilt Forge releases use glibc; build Forge from source on musl systems");
    }

    let current = env!("CARGO_PKG_VERSION");
    let current_version =
        parse_version(current).with_context(|| format!("invalid current version: {current}"))?;

    println!("Current version: {current}");
    println!("Checking latest release...");
    let tag = latest_tag()?;
    let latest_version =
        parse_version(&tag).with_context(|| format!("invalid release tag: {tag}"))?;

    if check {
        if latest_version > current_version {
            println!("Forge {tag} is available. Run `forge update` to install it.");
        } else if latest_version == current_version {
            println!("Forge {current} is up to date.");
        } else {
            println!("Current build {current} is newer than the latest release {tag}.");
        }
        return Ok(());
    }

    if latest_version == current_version && !force {
        println!("Forge {current} is up to date.");
        return Ok(());
    }
    if latest_version < current_version && !force {
        println!("Current build {current} is newer than the latest release {tag}; nothing to do.");
        return Ok(());
    }

    let target = target_triple()?;
    let asset = format!("forge-{target}.tar.gz");
    let download_root = format!("https://github.com/{REPOSITORY}/releases/download/{tag}");
    let asset_url = format!("{download_root}/{asset}");
    let sums_url = format!("{download_root}/SHA256SUMS");

    let temp = TempDir::new()?;
    let archive = temp.path().join(&asset);
    let sums = temp.path().join("SHA256SUMS");

    println!("Downloading {asset}...");
    download(&asset_url, &archive)?;
    println!("Downloading SHA256SUMS...");
    download(&sums_url, &sums)?;

    println!("Verifying checksum...");
    verify_checksum(&archive, &sums, &asset)?;

    println!("Extracting...");
    extract(&archive, temp.path())?;
    let new_binary = temp.path().join("forge");
    if !new_binary.is_file() {
        bail!("release archive does not contain a forge binary");
    }

    let current_exe = env::current_exe().context("cannot locate the running forge binary")?;
    println!("Installing to {}...", current_exe.display());
    replace_binary(&new_binary, &current_exe)?;

    println!("Updated Forge {current} -> {tag}.");
    println!("Restart Forge to use the new version.");
    Ok(())
}

fn latest_tag() -> Result<String> {
    let mut output = curl_latest(true);
    if !matches!(&output, Ok(out) if out.status.success()) {
        output = curl_latest(false);
    }
    let output = output.context("failed to run curl; forge update needs curl installed")?;
    if !output.status.success() {
        bail!(
            "could not reach GitHub to find the latest release (curl exited with {})",
            output.status
        );
    }

    let final_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let tag = final_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|part| part.split(['?', '#']).next().unwrap_or(part))
        .filter(|part| part.starts_with('v') && part.len() > 1)
        .with_context(|| format!("could not determine the latest release tag from {final_url}"))?;
    Ok(tag.to_string())
}

fn curl_latest(head: bool) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("curl");
    if head {
        command.arg("-fsSLI");
    } else {
        command.arg("-fsSL");
    }
    command
        .args(["-o", "/dev/null", "-w", "%{url_effective}"])
        .arg(format!("https://github.com/{REPOSITORY}/releases/latest"))
        .output()
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--connect-timeout", "15", "-o"])
        .arg(destination)
        .arg(url)
        .status()
        .context("failed to run curl; forge update needs curl installed")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    Ok(())
}

fn verify_checksum(archive: &Path, sums: &Path, asset: &str) -> Result<()> {
    let contents =
        fs::read_to_string(sums).with_context(|| format!("cannot read {}", sums.display()))?;
    let expected = contents
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?;
            (name == asset).then_some(hash)
        })
        .with_context(|| format!("{asset} is not listed in SHA256SUMS"))?;

    let actual = sha256(archive)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let output = match Command::new("sha256sum").arg(path).output() {
        Ok(output) if output.status.success() => output,
        _ => Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .context("sha256sum or shasum is required to verify the download")?,
    };
    if !output.status.success() {
        bail!("checksum command failed for {}", path.display());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() != 64 {
        bail!("unexpected checksum output for {}", path.display());
    }
    Ok(hash.to_string())
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .status()
        .context("failed to run tar; forge update needs tar installed")?;
    if !status.success() {
        bail!("failed to extract {}", archive.display());
    }
    Ok(())
}

fn replace_binary(new_binary: &Path, current_exe: &Path) -> Result<()> {
    let directory = current_exe
        .parent()
        .context("forge executable has no parent directory")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let staged = directory.join(format!(".forge-update-{}-{nonce}", std::process::id()));

    fs::copy(new_binary, &staged)
        .with_context(|| format!("cannot write to {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("cannot make {} executable", staged.display()))?;
    }

    if let Err(error) = fs::rename(&staged, current_exe) {
        let _ = fs::remove_file(&staged);
        return Err(error).with_context(|| {
            format!(
                "cannot replace {}; if Forge is in a system directory, reinstall it with the install script",
                current_exe.display()
            )
        });
    }
    Ok(())
}

fn target_triple() -> Result<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => bail!("no prebuilt release for {os}/{arch}; build Forge from source"),
    }
}

fn parse_version(input: &str) -> Option<Version> {
    let core = input.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some(Version(major, minor, patch))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!("forge-update-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path)
            .with_context(|| format!("cannot create temporary directory {}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_versions() {
        assert_eq!(parse_version("v0.1.0"), Some(Version(0, 1, 0)));
        assert_eq!(parse_version("0.1.2"), Some(Version(0, 1, 2)));
        assert_eq!(parse_version("v2.3"), Some(Version(2, 3, 0)));
        assert_eq!(parse_version("v2.3.4-rc.1"), Some(Version(2, 3, 4)));
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn orders_versions() {
        assert!(parse_version("v0.2.0") > parse_version("v0.1.9"));
        assert!(parse_version("v1.0.0") > parse_version("v0.99.99"));
        assert_eq!(parse_version("v0.1.0"), parse_version("0.1.0"));
    }
}
