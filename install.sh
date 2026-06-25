#!/bin/sh
# Compass one-line installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/QuirijnPolinoco/compass/main/install.sh | sh
#
# It downloads the prebuilt release binary for your platform, verifies its
# SHA-256 checksum, smoke-tests it, and installs `compass`. On most systems the
# target is ~/.local/bin; if that is not already on your PATH the script prints
# the exact line to add for *your* shell. Nothing is compiled.
#
# Environment knobs:
#   COMPASS_VERSION       pin a release tag, e.g. v0.6.0   (default: latest)
#   COMPASS_INSTALL_DIR   where to install                 (default: $HOME/.local/bin)
#   COMPASS_MUSL=1        force the static musl Linux build. Auto-detected on musl
#                         systems (Alpine, most Docker images); also used as an
#                         automatic fallback if the glibc build will not run here.
#
# Uninstall: remove the binary (default `rm -f ~/.local/bin/compass`) and delete
# any `export PATH=...` line you added. Nothing else is written.
#
# Safety: every statement lives inside a function and `main` is invoked only on
# the final line, so a truncated `curl | sh` download can never run a partial
# command. Downloads are HTTPS-only and checksum-verified before anything is
# installed.

set -eu

REPO="QuirijnPolinoco/compass"
BIN="compass"

say() { printf 'compass-install: %s\n' "$1" >&2; }
nl()  { printf '\n' >&2; }
err() { printf 'compass-install: error: %s\n' "$1" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1; }

# from_source_hint <reason> — abort with a from-source pointer for platforms
# that have no prebuilt binary.
from_source_hint() {
  err "$1
  No prebuilt binary for this platform. Install from source instead:
    cargo install --git https://github.com/QuirijnPolinoco/compass compass-cli"
}

# pick_tools — choose a downloader + checksum tool, or abort with a clear note.
pick_tools() {
  if need curl; then
    DL=curl
  elif need wget; then
    DL=wget
    # GNU wget understands --https-only / --secure-protocol; BusyBox wget (the
    # only wget on Alpine and many musl images) does not. Probe once so we can
    # harden the GNU case without breaking the musl/Alpine systems we support.
    if wget --help 2>&1 | grep -q -- '--https-only'; then
      WGET_HTTPS=1
    else
      WGET_HTTPS=0
    fi
  else
    err "need 'curl' or 'wget' to download the release"
  fi

  need tar || err "need 'tar' to unpack the release"

  if need sha256sum; then
    SHA=sha256sum
  elif need shasum; then
    SHA=shasum
  else
    err "need 'sha256sum' or 'shasum' to verify the download"
  fi
}

# is_musl — true if this Linux system uses musl libc (Alpine and most Docker
# base images), where a glibc binary cannot run *at all* — its dynamic loader
# /lib64/ld-linux-x86-64.so.2 is absent.
is_musl() {
  for l in /lib/ld-musl-*.so.1 /lib/libc.musl-*.so.1; do
    if [ -e "$l" ]; then return 0; fi
  done
  # `ldd --version` prints "musl libc" on musl and "GNU libc"/"GLIBC" on glibc.
  if need ldd && ldd --version 2>&1 | grep -qi musl; then
    return 0
  fi
  return 1
}

# detect_target — map `uname` output to the exact release target triple, setting
# TARGET and (for glibc Linux) TARGET_FALLBACK to the static musl build so a
# libc-incompatible binary can be retried automatically after the smoke test.
detect_target() {
  TARGET_FALLBACK=""
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux)
      case "$arch" in
        x86_64 | amd64)
          if [ "${COMPASS_MUSL:-0}" = 1 ] || is_musl; then
            TARGET="x86_64-unknown-linux-musl"
          else
            TARGET="x86_64-unknown-linux-gnu"
            # The gnu build is compiled on a modern CI runner; on older glibc
            # (Ubuntu 20.04, Debian 11, RHEL/Rocky 8, Amazon Linux 2) it dies
            # with "GLIBC_2.3x not found". The musl build is statically linked
            # and runs everywhere, so fall back to it if gnu won't execute here.
            TARGET_FALLBACK="x86_64-unknown-linux-musl"
          fi
          ;;
        aarch64 | arm64)
          from_source_hint "linux/arm64 is not yet built as a release binary." ;;
        *)
          from_source_hint "unsupported Linux architecture '$arch'." ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        arm64 | aarch64) TARGET="aarch64-apple-darwin" ;;
        x86_64 | amd64)  TARGET="x86_64-apple-darwin" ;;
        *) from_source_hint "unsupported macOS architecture '$arch'." ;;
      esac
      ;;
    *)
      err "unsupported OS '$os'.
  On Windows, use the PowerShell installer instead:
    irm https://raw.githubusercontent.com/QuirijnPolinoco/compass/main/install.ps1 | iex
  Otherwise install from source:
    cargo install --git https://github.com/QuirijnPolinoco/compass compass-cli"
      ;;
  esac
}

# download <url> <out> — fetch <url> to <out> over HTTPS only.
download() {
  case "$1" in
    https://*) : ;;
    *) err "refusing to download non-HTTPS URL: $1" ;;
  esac
  if [ "$DL" = curl ]; then
    curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
  elif [ "${WGET_HTTPS:-0}" = 1 ]; then
    # --https-only refuses an http:// redirect; TLS floor matches the curl branch.
    wget --https-only --secure-protocol=TLSv1_2 -q -O "$2" "$1"
  else
    wget -q -O "$2" "$1"
  fi
}

# sha256_of <file> — print the lowercase hex digest of <file>.
sha256_of() {
  if [ "$SHA" = sha256sum ]; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# verify_checksum <file> <sha256file> — abort unless they match.
verify_checksum() {
  expected="$(awk '{print $1}' "$2" | tr '[:upper:]' '[:lower:]')"
  actual="$(sha256_of "$1" | tr '[:upper:]' '[:lower:]')"
  [ -n "$expected" ] || err "checksum file '$2' was empty"
  if [ "$expected" != "$actual" ]; then
    err "checksum mismatch for $(basename "$1")
  expected: $expected
  actual:   $actual
  Refusing to install a binary that does not match its published checksum."
  fi
}

# on_path <dir> — true if <dir> is an entry in PATH.
on_path() {
  case ":${PATH:-}:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# path_hint <dir> — print shell-correct PATH instructions. macOS defaults to zsh
# (which never reads ~/.profile), and bash reads different files on macOS vs
# Linux, so target the file the user's actual shell loads.
path_hint() {
  dir="$1"
  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    zsh)
      rc="$HOME/.zshrc"
      line="export PATH=\"$dir:\$PATH\""
      ;;
    bash)
      # macOS Terminal opens login shells (~/.bash_profile); Linux terminals
      # open interactive non-login shells (~/.bashrc).
      if [ "$(uname -s)" = Darwin ]; then
        rc="$HOME/.bash_profile"
      else
        rc="$HOME/.bashrc"
      fi
      line="export PATH=\"$dir:\$PATH\""
      ;;
    fish)
      rc="$HOME/.config/fish/config.fish"
      line="fish_add_path \"$dir\""
      ;;
    *)
      rc="$HOME/.profile"
      line="export PATH=\"$dir:\$PATH\""
      ;;
  esac
  say "NOTE: $dir is not on your PATH. Add it for your shell ($shell_name):"
  say "  echo '$line' >> $rc"
  say "  then reopen your terminal (or run that line now to use it immediately)"
}

# ensure_writable <dir> — create <dir> if missing and confirm we can write to it,
# turning a raw 'Permission denied' under `set -e` into a friendly hint. This is
# the natural failure when COMPASS_INSTALL_DIR points at a root-owned dir such as
# /usr/local/bin.
ensure_writable() {
  d="$1"
  if [ -d "$d" ]; then
    [ -w "$d" ] || err "install dir is not writable: $d
  Re-run under sudo to install into a system directory, or choose a dir you own:
    COMPASS_INSTALL_DIR=\"\$HOME/.local/bin\" sh -c '...'"
  else
    mkdir -p "$d" 2>/dev/null || err "cannot create install dir: $d
  Its parent is not writable. Re-run under sudo, or choose a dir you own:
    COMPASS_INSTALL_DIR=\"\$HOME/.local/bin\" sh -c '...'"
  fi
}

# fetch_install <target> — download, verify, unpack, and install the binary for
# <target> to $dest. Reused for the musl fallback. Relies on $base/$tmp/$dest/
# $version set by main (sh functions share the global scope).
fetch_install() {
  target="$1"
  asset="${BIN}-${target}.tar.gz"

  say "downloading $asset"
  download "$base/$asset" "$tmp/$asset" \
    || err "download failed (is '$version' a real release for $target?)"
  download "$base/$asset.sha256" "$tmp/$asset.sha256" \
    || err "could not download the checksum for $asset"

  say "verifying checksum"
  verify_checksum "$tmp/$asset" "$tmp/$asset.sha256"

  say "unpacking"
  tar -xzf "$tmp/$asset" -C "$tmp"
  [ -f "$tmp/$BIN" ] || err "archive '$asset' did not contain '$BIN'"
  chmod +x "$tmp/$BIN"

  mv -f "$tmp/$BIN" "$dest"
}

# smoke_ok — true if the freshly installed binary actually runs here. A wrong
# arch or libc build installs fine but cannot execute.
smoke_ok() {
  "$dest" --help >/dev/null 2>&1
}

main() {
  pick_tools
  detect_target

  version="${COMPASS_VERSION:-latest}"
  # Release tags are v-prefixed (v0.6.0); accept a bare COMPASS_VERSION=0.6.0 and
  # fix it up so it doesn't 404 against /releases/download/0.6.0/...
  case "$version" in
    latest | v*) : ;;
    [0-9]*) say "note: release tags are v-prefixed; using v$version"; version="v$version" ;;
  esac

  if [ "$version" = latest ]; then
    base="https://github.com/${REPO}/releases/latest/download"
  else
    base="https://github.com/${REPO}/releases/download/${version}"
  fi
  install_dir="${COMPASS_INSTALL_DIR:-$HOME/.local/bin}"

  say "installing compass ($version) for $TARGET"

  tmp="$(mktemp -d 2>/dev/null || mktemp -d -t compass)"
  trap 'rm -rf "$tmp"' EXIT INT TERM HUP

  ensure_writable "$install_dir"
  dest="$install_dir/$BIN"

  fetch_install "$TARGET"
  say "installed compass to $dest"

  # Smoke-test before declaring success. If the primary (glibc) build won't run
  # and a static musl fallback exists, retry with it instead of leaving the user
  # a broken binary that the installer claimed was fine.
  if ! smoke_ok; then
    if [ -n "$TARGET_FALLBACK" ] && [ "$TARGET" != "$TARGET_FALLBACK" ]; then
      say "the $TARGET build does not run here (older or musl libc); retrying with $TARGET_FALLBACK"
      TARGET="$TARGET_FALLBACK"
      fetch_install "$TARGET"
      say "installed compass to $dest ($TARGET)"
    fi
    if ! smoke_ok; then
      err "installed $dest but it will not run on this system (architecture/libc mismatch).
  Install from source instead:
    cargo install --git https://github.com/QuirijnPolinoco/compass compass-cli"
    fi
  fi

  if ! on_path "$install_dir"; then
    nl
    path_hint "$install_dir"
  fi

  nl
  say "done - run '$BIN --help' to get started."
}

main "$@"
