#!/usr/bin/env sh
# install.sh — one command. That's the product.
#
#   curl -fsSL https://astridos.org/install.sh | sh
#
# Quiet: only checkmarks of what ran. Does CLI + base + detected host
# plugins (marketplace) + host capsules (astrid init). Re-run upgrades.
set -eu

ORACLES_REPO="${ASTRID_ORACLES_REPO:-unicity-astrid/oracles}"
DISTRO_BASE="${ASTRID_ORACLES_DISTRO_BASE:-https://raw.githubusercontent.com/${ORACLES_REPO}/main/distros}"
ASTRID_RELEASE_REPO="${ASTRID_RELEASE_REPO:-unicity-astrid/astrid}"
ASTRID_MANAGED_BIN="${ASTRID_HOME:-$HOME/.astrid}/bin"
BREW_TAP="${ASTRID_BREW_TAP:-unicity-astrid/tap}"
BREW_FORMULA="${ASTRID_BREW_FORMULA:-astrid}"

NO_BREW=0
BASE_ONLY=0
ALL_HOSTS=0
REQUESTED_HOSTS=""
BIN_ROOT="${ASTRID_BIN_ROOT:-}"
ASTRID=""

have() { command -v "$1" >/dev/null 2>&1; }
say()  { printf '%s\n' "$*"; }
die()  { say "✗ $*" >&2; exit 1; }
ok()   { say "✓ $*"; }
fail() { say "✗ $*"; }

usage() {
  cat <<'EOF'
curl -fsSL https://astridos.org/install.sh | sh

One command: CLI, base home, plugins + capsules for hosts on this machine.

  --host NAME   only this host (claude|grok|codex), repeatable
  --all         every host
  --base-only   skip host plugins/capsules
  --no-brew     never use Homebrew
  --bin-root D  use astrid from D
  -h, --help
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      h="${1:-}"
      case "$h" in
        claude|grok|codex) REQUESTED_HOSTS="${REQUESTED_HOSTS} ${h}" ;;
        *) die "unknown host '$h'" ;;
      esac
      ;;
    --all) ALL_HOSTS=1 ;;
    --base-only) BASE_ONLY=1 ;;
    --no-brew) NO_BREW=1 ;;
    --bin-root)
      shift
      BIN_ROOT="${1:-}"
      [ -n "$BIN_ROOT" ] || die "--bin-root needs a path"
      ;;
    --yes|-y|--upgrade|--verbose|-v|--skip-init) ;; # accepted for compat
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done

if [ -z "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ] && have gh; then
  _tok="$(gh auth token 2>/dev/null || true)"
  [ -n "$_tok" ] && export GH_TOKEN="$_tok"
fi

has_pair() { [ -n "$1" ] && [ -x "$1/astrid" ] && [ -x "$1/astrid-daemon" ]; }

platform_target() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  case "${os}/${arch}" in
    Darwin/arm64|Darwin/aarch64) printf 'aarch64-apple-darwin\n' ;;
    Darwin/x86_64)               printf 'x86_64-apple-darwin\n' ;;
    Linux/x86_64|Linux/amd64)    printf 'x86_64-unknown-linux-gnu\n' ;;
    Linux/aarch64|Linux/arm64)   printf 'aarch64-unknown-linux-gnu\n' ;;
    *) die "unsupported platform ${os}/${arch}" ;;
  esac
}

sha256_file() {
  if have sha256sum; then sha256sum "$1" | awk '{ print $1 }'
  elif have shasum; then shasum -a 256 "$1" | awk '{ print $1 }'
  else die "need sha256sum or shasum"
  fi
}

install_from_github() {
  target="$(platform_target)"
  tmp="$(mktemp -d 2>/dev/null || mktemp -d -t astrid-install)"
  # shellcheck disable=SC2064
  trap 'rm -rf "$tmp" 2>/dev/null || true' EXIT

  if [ -n "${ASTRID_VERSION:-}" ]; then
    tag="v${ASTRID_VERSION#v}"
  else
    have curl || die "curl required"
    api="https://api.github.com/repos/${ASTRID_RELEASE_REPO}/releases/latest"
    meta="$(curl -fsSL --max-time 30 \
      ${GH_TOKEN:+-H "Authorization: Bearer ${GH_TOKEN}"} \
      ${GITHUB_TOKEN:+-H "Authorization: Bearer ${GITHUB_TOKEN}"} \
      "$api")" || die "could not query GitHub releases"
    tag="$(printf '%s' "$meta" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
    [ -n "$tag" ] || die "latest release has no tag_name"
  fi
  version="${tag#v}"
  asset="astrid-${version}-${target}.tar.gz"
  base="https://github.com/${ASTRID_RELEASE_REPO}/releases/download/${tag}"

  curl -fsSL --max-time 120 -o "$tmp/$asset" "${base}/${asset}" || die "download failed"
  curl -fsSL --max-time 30 -o "$tmp/SHA256SUMS.txt" "${base}/SHA256SUMS.txt" || die "no SHA256SUMS"
  expected="$(awk -v a="$asset" '$2 == a || $2 == "./"a || index($0, a) { print $1; exit }' "$tmp/SHA256SUMS.txt")"
  [ -n "$expected" ] || die "no checksum for $asset"
  actual="$(sha256_file "$tmp/$asset")"
  [ "$expected" = "$actual" ] || die "checksum mismatch"

  mkdir -p "$ASTRID_MANAGED_BIN"
  tar -xzf "$tmp/$asset" -C "$tmp"
  found=""
  for d in "$tmp"/* "$tmp"; do
    [ -d "$d" ] || continue
    if [ -x "$d/astrid" ] && [ -x "$d/astrid-daemon" ]; then found="$d"; break; fi
  done
  [ -n "$found" ] || die "archive missing astrid + astrid-daemon"
  for b in astrid astrid-daemon astrid-build; do
    [ -x "$found/$b" ] && cp -f "$found/$b" "$ASTRID_MANAGED_BIN/$b" && chmod 755 "$ASTRID_MANAGED_BIN/$b"
  done
  export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
  ASTRID="$ASTRID_MANAGED_BIN/astrid"
  export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"

  case "$(uname -s 2>/dev/null)" in Darwin) path_rc="$HOME/.zprofile" ;; *) path_rc="$HOME/.profile" ;; esac
  if [ -n "$path_rc" ] && ! grep -qF '.astrid/bin' "$path_rc" 2>/dev/null; then
    printf '\n# Astrid CLI\nexport PATH="%s:$PATH"\n' "$ASTRID_MANAGED_BIN" >> "$path_rc"
  fi
  trap - EXIT
  rm -rf "$tmp" 2>/dev/null || true
}

ensure_cli() {
  if [ -n "$BIN_ROOT" ]; then
    has_pair "$BIN_ROOT" || die "--bin-root incomplete"
    ASTRID="$BIN_ROOT/astrid"
    export ASTRID_BIN_ROOT="$BIN_ROOT"
    ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    return 0
  fi
  if [ -n "${ASTRID_BIN:-}" ] && [ -x "$ASTRID_BIN" ]; then
    ASTRID="$ASTRID_BIN"
    ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    return 0
  fi

  if has_pair "$ASTRID_MANAGED_BIN"; then
    export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
    ASTRID="$ASTRID_MANAGED_BIN/astrid"
    export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"
  elif have astrid; then
    ASTRID="$(command -v astrid)"
  else
    ASTRID=""
  fi

  if [ -n "$ASTRID" ] && [ -x "$ASTRID" ]; then
    if "$ASTRID" update -y >/dev/null 2>&1; then
      if has_pair "$ASTRID_MANAGED_BIN"; then
        export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
        ASTRID="$ASTRID_MANAGED_BIN/astrid"
      elif have astrid; then
        ASTRID="$(command -v astrid)"
      fi
      ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    else
      ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    fi
    return 0
  fi

  if install_from_github; then
    ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    return 0
  fi
  if [ "$NO_BREW" -eq 0 ] && have brew; then
    brew tap "$BREW_TAP" >/dev/null 2>&1 || true
    brew install "${BREW_TAP}/${BREW_FORMULA}" >/dev/null 2>&1 \
      || brew install "$BREW_FORMULA" >/dev/null 2>&1 \
      || die "Homebrew install failed"
    ASTRID="$(command -v astrid)" || die "no astrid on PATH after brew"
    ok "CLI $($ASTRID --version 2>/dev/null | head -n1)"
    return 0
  fi
  die "could not install Astrid"
}

ensure_base() {
  home="${ASTRID_HOME:-$HOME/.astrid}"
  if [ -d "$home/home/default" ] || [ -f "$home/config.toml" ] \
    || [ -f "$home/home/default/.config/distro.lock" ] \
    || [ -f "$home/home/default/.config/Distro.lock" ]; then
    ok "base home"
    return 0
  fi
  if "$ASTRID" init -y >/dev/null 2>&1; then
    ok "base home"
  else
    fail "base home (astrid init) — try: astrid doctor"
  fi
}

detect_hosts() {
  hosts=""
  if [ "$BASE_ONLY" -eq 1 ]; then printf '%s' ""; return 0; fi
  if [ "$ALL_HOSTS" -eq 1 ]; then printf '%s' "claude grok codex"; return 0; fi
  if [ -n "$REQUESTED_HOSTS" ]; then printf '%s' "$REQUESTED_HOSTS"; return 0; fi
  if have claude || [ -x "${HOME}/.claude/local/claude" ] || [ -d "${HOME}/.claude" ]; then
    hosts="${hosts} claude"
  fi
  if have grok || [ -d "${HOME}/.grok" ]; then hosts="${hosts} grok"; fi
  if have codex || [ -d "${HOME}/.codex" ]; then hosts="${hosts} codex"; fi
  printf '%s' "$hosts"
}

distro_url() { printf '%s/%s.toml\n' "$DISTRO_BASE" "$1"; }

principal_for() {
  case "$1" in
    claude) printf 'claude-code\n' ;;
    grok)   printf 'grok-code\n' ;;
    codex)  printf 'codex-code\n' ;;
  esac
}

pretty() {
  case "$1" in
    claude) printf 'Claude Code\n' ;;
    grok)   printf 'Grok Build\n' ;;
    codex)  printf 'Codex\n' ;;
  esac
}

# Best-effort marketplace install. Host CLIs differ; failures are soft.
install_plugin() {
  host="$1"
  label="$(pretty "$host")"
  case "$host" in
    claude)
      if ! have claude && [ ! -x "${HOME}/.claude/local/claude" ]; then
        fail "$label plugin (no claude CLI)"
        return 1
      fi
      claude_bin="$(command -v claude 2>/dev/null || true)"
      [ -n "$claude_bin" ] || claude_bin="${HOME}/.claude/local/claude"
      if "$claude_bin" plugin marketplace add "$ORACLES_REPO" >/dev/null 2>&1 \
        && "$claude_bin" plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      elif "$claude_bin" plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      else
        fail "$label plugin"
        return 1
      fi
      ;;
    grok)
      if ! have grok; then
        fail "$label plugin (no grok CLI)"
        return 1
      fi
      if grok plugin marketplace add "$ORACLES_REPO" >/dev/null 2>&1 \
        && grok plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      elif grok plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      else
        fail "$label plugin"
        return 1
      fi
      ;;
    codex)
      if ! have codex; then
        fail "$label plugin (no codex CLI)"
        return 1
      fi
      if codex plugin marketplace add "$ORACLES_REPO" >/dev/null 2>&1 \
        && codex plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      elif codex plugin install astrid@astrid-oracles >/dev/null 2>&1; then
        ok "$label plugin"
      else
        fail "$label plugin"
        return 1
      fi
      ;;
  esac
}

install_capsules() {
  host="$1"
  p="$(principal_for "$host")"
  d="$(distro_url "$host")"
  label="$(pretty "$host")"
  "$ASTRID" init --distro "$d" -y >/dev/null 2>&1 || true
  if "$ASTRID" init --distro "$d" --principal "$p" -y >/dev/null 2>&1; then
    ok "$label capsules ($p)"
  else
    fail "$label capsules ($p) — need GH_TOKEN=\$(gh auth token)?"
  fi
}

wire_host() {
  install_plugin "$1" || true
  install_capsules "$1"
}

main() {
  ensure_cli
  ensure_base

  hosts="$(detect_hosts)"
  # shellcheck disable=SC2086
  set -- $hosts

  if [ "$#" -eq 0 ]; then
    ok "hosts (none detected — base only)"
  else
    for h in "$@"; do
      wire_host "$h"
    done
  fi
}

main
