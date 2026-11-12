#!/bin/sh
# Install Forge from a GitHub release.
#
#   curl -fsSL https://creature-studio.github.io/forge/install.sh | sh
#
# Environment overrides:
#   FORGE_VERSION      release tag to install (default: latest, e.g. v0.1.0)
#   FORGE_INSTALL_DIR  install directory (default: ~/.local/bin)
set -eu

REPOSITORY="creature-studio/forge"
BINARY="forge"

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found in PATH"
}

# --- platform -------------------------------------------------------------

os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)

case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) fail "unsupported operating system: $os (Forge ships Linux and macOS builds)" ;;
esac

case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) fail "unsupported CPU architecture: $arch" ;;
esac

target="${arch_part}-${os_part}"

# --- tools ----------------------------------------------------------------

need tar
need uname

if command -v curl >/dev/null 2>&1; then
    fetch_stdout() {
        curl -fsSL "$1"
    }
    fetch_latest_redirect() {
        curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPOSITORY}/releases/latest"
    }
    download() {
        curl -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch_stdout() {
        wget -q -O - "$1"
    }
    fetch_latest_redirect() {
        # wget cannot expose the redirect target portably, so ask the API instead.
        fetch_stdout "https://api.github.com/repos/${REPOSITORY}/releases/latest" |
            sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -n 1
    }
    download() {
        wget -q -O "$2" "$1"
    }
else
    fail "curl or wget is required to download Forge"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256() {
        sha256sum "$1" | cut -d' ' -f1
    }
elif command -v shasum >/dev/null 2>&1; then
    sha256() {
        shasum -a 256 "$1" | cut -d' ' -f1
    }
else
    fail "sha256sum or shasum is required to verify the download"
fi

# --- version --------------------------------------------------------------

tag=${FORGE_VERSION:-}
if [ -z "$tag" ]; then
    say "Looking up the latest Forge release..."
    latest=$(fetch_latest_redirect || true)
    tag=$(printf '%s' "$latest" | tr -d '\r' | sed -n 's#.*/tag/\([^/?#]*\).*#\1#p')
    if [ -z "$tag" ]; then
        # Fall back to the API response shape ({"tag_name": "vX.Y.Z", ...}).
        tag=$(printf '%s' "$latest" | tr -d '\r' | sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -n 1)
    fi
    [ -n "$tag" ] || fail "could not determine the latest Forge release; set FORGE_VERSION=vX.Y.Z and retry"
fi
case "$tag" in
    v*) ;;
    [0-9]*) tag="v${tag}" ;;
    *) fail "unexpected release tag: $tag" ;;
esac

# --- download -------------------------------------------------------------

asset="${BINARY}-${target}.tar.gz"
base="https://github.com/${REPOSITORY}/releases/download/${tag}"

tmpdir=$(mktemp -d 2>/dev/null || mktemp -d -t forge)
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT INT TERM

say "Downloading ${asset} (${tag})..."
download "${base}/${asset}" "${tmpdir}/${asset}" || fail "download failed: ${base}/${asset}"
download "${base}/SHA256SUMS" "${tmpdir}/SHA256SUMS" || fail "download failed: ${base}/SHA256SUMS"

expected=$(awk -v name="$asset" '$2 == name { print $1; exit }' "${tmpdir}/SHA256SUMS")
[ -n "$expected" ] || fail "${asset} is not listed in SHA256SUMS"

actual=$(sha256 "${tmpdir}/${asset}")
if [ "$expected" != "$actual" ]; then
    fail "checksum mismatch for ${asset}: expected ${expected}, got ${actual}"
fi
say "Checksum verified."

tar -xzf "${tmpdir}/${asset}" -C "$tmpdir" || fail "failed to extract ${asset}"
[ -f "${tmpdir}/${BINARY}" ] || fail "release archive does not contain a ${BINARY} binary"

# --- install --------------------------------------------------------------

install_dir=${FORGE_INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}
mkdir -p "$install_dir" || fail "cannot create install directory: $install_dir"

if command -v install >/dev/null 2>&1; then
    install -m 0755 "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}" || fail "cannot write to ${install_dir}"
else
    cp "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}" || fail "cannot write to ${install_dir}"
    chmod 0755 "${install_dir}/${BINARY}" || fail "cannot make ${install_dir}/${BINARY} executable"
fi

say "Installed ${BINARY} ${tag} to ${install_dir}/${BINARY}"

case ":$PATH:" in
    *":${install_dir}:"*) ;;
    *)
        say ""
        say "${install_dir} is not in your PATH. Add it with:"
        say "  echo 'export PATH=\"${install_dir}:\$PATH\"' >> ~/.profile"
        ;;
esac

say ""
say "Run '${BINARY}' in any project directory to start, and '${BINARY} update' to upgrade later."
