#!/usr/bin/env sh
# install.sh — one-command Astrid installer (base OS + optional host plugins)
#
# Website / GitHub Pages / raw:
#   curl -fsSL https://astridos.org/install.sh | sh
#
# Product model:
#   * Base Astrid is a complete install (daemon, CLI, default principal).
#   * Host plugins (Claude / Grok / Codex) are optional on top of base Astrid.
#
# Behaviour (least surprise) — same command installs *and* upgrades:
#   1. Existing CLI → `astrid update -y` (Astrid's own updater: brew/cargo/managed)
#      Fresh machine → GitHub Releases → ~/.astrid/bin (brew last-resort)
#   2. Base home when needed (`astrid init -y`)
#   3. Detect Claude Code / Grok / Codex
#   4. Per host: `astrid init --distro … --principal …` — idempotent via Distro.lock
#      (fresh lock = no-op; no lock / distro version bump = provision)
#   5. If none detected → stop at base Astrid (success)
#   6. Re-run after adding Grok → detects + first-time init for that principal only
#
# Flags:
#   --host claude|grok|codex   wire only this host (repeatable)
#   --all                      wire every host (power users / demos)
#   --base-only                skip host plugins
#   --upgrade                  force re-apply base init (re-run already refreshes CLI + hosts)
#   --yes / -y                 non-interactive (default when not a TTY)
#   --no-brew                  never invoke Homebrew
#   --bin-root PATH            use this directory for astrid + astrid-daemon
#   --skip-init                install/detect only; do not run astrid init
#   -h / --help

set -eu

# ---------------------------------------------------------------------------
# Defaults — override with env for mirrors / forks
# ---------------------------------------------------------------------------
ORACLES_REPO="${ASTRID_ORACLES_REPO:-unicity-astrid/oracles}"
ORACLES_REF="${ASTRID_ORACLES_REF:-main}"
DISTRO_BASE="${ASTRID_ORACLES_DISTRO_BASE:-https://raw.githubusercontent.com/${ORACLES_REPO}/${ORACLES_REF}/distros}"
BREW_TAP="${ASTRID_BREW_TAP:-unicity-astrid/tap}"
BREW_FORMULA="${ASTRID_BREW_FORMULA:-astrid}"

# ---------------------------------------------------------------------------
# UI
# ---------------------------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_BOLD='\033[1m'
  C_DIM='\033[2m'
  C_GREEN='\033[32m'
  C_YELLOW='\033[33m'
  C_CYAN='\033[36m'
  C_RED='\033[31m'
  C_RESET='\033[0m'
else
  C_BOLD='' C_DIM='' C_GREEN='' C_YELLOW='' C_CYAN='' C_RED='' C_RESET=''
fi

say()  { printf '%b\n' "$*"; }
info() { say "${C_CYAN}→${C_RESET} $*"; }
ok()   { say "${C_GREEN}✓${C_RESET} $*"; }
warn() { say "${C_YELLOW}!${C_RESET} $*"; }
err()  { say "${C_RED}✗${C_RESET} $*" >&2; }
die()  { err "$*"; exit 1; }
header() {
  say ""
  say "${C_BOLD}$*${C_RESET}"
  say "${C_DIM}$(printf '%*s' "${#1}" '' | tr ' ' '─')${C_RESET}"
}

usage() {
  cat <<'EOF'
Astrid installer (base + optional host plugins)

  curl -fsSL https://astridos.org/install.sh | sh

Same one command installs and upgrades: refreshes the CLI when managed under
~/.astrid/bin, re-detects hosts, and wires any new ones (e.g. you installed Grok).

Options:
  --host NAME     Wire claude | grok | codex (repeatable)
  --all           Wire every host (not the default)
  --base-only     Base Astrid only (skip host plugins)
  --upgrade       Force re-apply base init (re-run already refreshes CLI + hosts)
  --yes, -y       Non-interactive (auto-yes; default when stdin is not a TTY)
  --no-brew       Do not fall back to Homebrew if GitHub Releases install fails
  --bin-root DIR  Prefer astrid + astrid-daemon from DIR
  --skip-init     Skip astrid init / principal provisioning
  -h, --help      Show this help

Env:
  ASTRID_ORACLES_REPO          default unicity-astrid/oracles
  ASTRID_ORACLES_REF           default main
  ASTRID_ORACLES_DISTRO_BASE   raw base URL for distro tomls
  ASTRID_BIN_ROOT              same as --bin-root
EOF
}

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------
YES=0
NO_BREW=0
SKIP_INIT=0
ALL_HOSTS=0
BASE_ONLY=0
UPGRADE=0
BIN_ROOT="${ASTRID_BIN_ROOT:-}"
# space-separated list of hosts requested via --host
REQUESTED_HOSTS=""

# Non-interactive when not a TTY (curl | sh)
if [ ! -t 0 ]; then
  YES=1
fi

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      h="${1:-}"
      case "$h" in
        claude|grok|codex) REQUESTED_HOSTS="${REQUESTED_HOSTS} ${h}" ;;
        *) die "unknown host '$h' (want claude|grok|codex)" ;;
      esac
      ;;
    --all) ALL_HOSTS=1 ;;
    --base-only) BASE_ONLY=1 ;;
    --upgrade) UPGRADE=1 ;;
    --yes|-y) YES=1 ;;
    --no-brew) NO_BREW=1 ;;
    --bin-root)
      shift
      BIN_ROOT="${1:-}"
      [ -n "$BIN_ROOT" ] || die "--bin-root needs a path"
      ;;
    --skip-init) SKIP_INIT=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------------------
# Detect hosts already on the machine
# ---------------------------------------------------------------------------
have_cmd() { command -v "$1" >/dev/null 2>&1; }

detect_claude() {
  have_cmd claude && return 0
  [ -x "${HOME}/.claude/local/claude" ] && return 0
  [ -d "${HOME}/.claude" ] && return 0
  return 1
}

detect_grok() {
  have_cmd grok && return 0
  [ -d "${HOME}/.grok" ] && return 0
  return 1
}

detect_codex() {
  have_cmd codex && return 0
  [ -d "${HOME}/.codex" ] && return 0
  return 1
}

# ---------------------------------------------------------------------------
# Astrid binary resolution — GitHub releases first (macOS + Linux)
# ---------------------------------------------------------------------------
# Brew is optional and uncommon on Linux. Primary path matches `astrid update`
# and setup-astrid: download the release tarball into ~/.astrid/bin.
ASTRID_RELEASE_REPO="${ASTRID_RELEASE_REPO:-unicity-astrid/astrid}"
ASTRID_MANAGED_BIN="${ASTRID_HOME:-$HOME/.astrid}/bin"

has_pair() {
  root="$1"
  [ -n "$root" ] && [ -x "$root/astrid" ] && [ -x "$root/astrid-daemon" ]
}

platform_target() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  case "${os}/${arch}" in
    Darwin/arm64|Darwin/aarch64) printf 'aarch64-apple-darwin\n' ;;
    Darwin/x86_64)               printf 'x86_64-apple-darwin\n' ;;
    Linux/x86_64|Linux/amd64)    printf 'x86_64-unknown-linux-gnu\n' ;;
    Linux/aarch64|Linux/arm64)   printf 'aarch64-unknown-linux-gnu\n' ;;
    *)
      die "unsupported platform ${os}/${arch} — Astrid ships macOS and Linux (x86_64/arm64) release binaries"
      ;;
  esac
}

sha256_file() {
  if have_cmd sha256sum; then
    sha256sum "$1" | awk '{ print $1 }'
  elif have_cmd shasum; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    die "need sha256sum or shasum to verify the release archive"
  fi
}

# Download latest (or ASTRID_VERSION) release into ~/.astrid/bin.
install_astrid_from_github() {
  target="$(platform_target)"
  tmp="$(mktemp -d 2>/dev/null || mktemp -d -t astrid-install)"
  # shellcheck disable=SC2064

  info "fetching Astrid release metadata from GitHub (${ASTRID_RELEASE_REPO})..."

  if [ -n "${ASTRID_VERSION:-}" ]; then
    tag="v${ASTRID_VERSION#v}"
  elif have_cmd curl; then
    api="https://api.github.com/repos/${ASTRID_RELEASE_REPO}/releases/latest"
    meta="$(curl -fsSL --max-time 30 \
      ${GITHUB_TOKEN:+-H "Authorization: Bearer ${GITHUB_TOKEN}"} \
      "$api")" || {
        warn "could not query GitHub releases (network / rate limit)"
        rm -rf "$tmp" 2>/dev/null || true
        return 1
      }
    tag="$(printf '%s' "$meta" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
    [ -n "$tag" ] || {
      warn "latest release has no tag_name"
      rm -rf "$tmp" 2>/dev/null || true
      return 1
    }
  else
    warn "curl is required to download Astrid from GitHub releases"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  fi
  version="${tag#v}"
  asset="astrid-${version}-${target}.tar.gz"
  base="https://github.com/${ASTRID_RELEASE_REPO}/releases/download/${tag}"

  info "downloading ${asset}..."
  curl -fsSL --max-time 120 -o "$tmp/$asset" "${base}/${asset}" \
    || {
      warn "download failed: ${base}/${asset}"
      rm -rf "$tmp" 2>/dev/null || true
      return 1
    }
  curl -fsSL --max-time 30 -o "$tmp/SHA256SUMS.txt" "${base}/SHA256SUMS.txt" \
    || {
      warn "could not download SHA256SUMS.txt"
      rm -rf "$tmp" 2>/dev/null || true
      return 1
    }

  expected="$(awk -v a="$asset" '$2 == a || $2 == "./"a || index($0, a) { print $1; exit }' "$tmp/SHA256SUMS.txt")"
  [ -n "$expected" ] || {
    warn "no checksum for $asset in SHA256SUMS.txt"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  }
  actual="$(sha256_file "$tmp/$asset")"
  if [ "$expected" != "$actual" ]; then
    warn "checksum mismatch for $asset (expected $expected, got $actual)"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  fi
  ok "SHA256 verified for $asset"

  info "extracting into ${ASTRID_MANAGED_BIN}..."
  tar -xzf "$tmp/$asset" -C "$tmp"
  # Archive layout: astrid-${version}-${target}/astrid ...
  src=""
  for cand in "$tmp/astrid-${version}-${target}" "$tmp"; do
    if [ -f "$cand/astrid" ]; then src="$cand"; break; fi
  done
  [ -n "$src" ] || {
    warn "archive missing astrid binary"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  }

  mkdir -p "$ASTRID_MANAGED_BIN"
  for bin in astrid astrid-daemon astrid-build astrid-emit; do
    if [ -f "$src/$bin" ]; then
      cp "$src/$bin" "$ASTRID_MANAGED_BIN/$bin"
      chmod +x "$ASTRID_MANAGED_BIN/$bin"
    fi
  done
  [ -x "$ASTRID_MANAGED_BIN/astrid" ] || {
    warn "install failed: $ASTRID_MANAGED_BIN/astrid not executable"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  }
  [ -x "$ASTRID_MANAGED_BIN/astrid-daemon" ] || {
    warn "install failed: astrid-daemon missing"
    rm -rf "$tmp" 2>/dev/null || true
    return 1
  }

  # Current shell
  export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
  ASTRID="$ASTRID_MANAGED_BIN/astrid"
  export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"

  # Persist PATH for future logins (idempotent)
  path_line='export PATH="$HOME/.astrid/bin:$PATH"'
  for rc in "$HOME/.zprofile" "$HOME/.zshrc" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.profile"; do
    [ -f "$rc" ] || continue
    if grep -qF '.astrid/bin' "$rc" 2>/dev/null; then
      ok "PATH already mentions .astrid/bin in $rc"
      break
    fi
  done
  # Prefer zsh on macOS, bash profile on linux, else .profile
  path_rc=""
  case "$(uname -s 2>/dev/null)" in
    Darwin) path_rc="$HOME/.zprofile" ;;
    *)      path_rc="$HOME/.profile" ;;
  esac
  if [ -n "$path_rc" ] && ! grep -qF '.astrid/bin' "$path_rc" 2>/dev/null; then
    printf '\n# Astrid CLI (github.com/%s releases)\n%s\n' "$ASTRID_RELEASE_REPO" "$path_line" >> "$path_rc"
    ok "added ~/.astrid/bin to PATH in $path_rc (new shells)"
  fi

  ok "installed Astrid ${version} → ${ASTRID_MANAGED_BIN}"
  rm -rf "$tmp" 2>/dev/null || true
  return 0
}

install_astrid_from_brew() {
  have_cmd brew || return 1
  info "falling back to Homebrew..."
  brew tap "$BREW_TAP" 2>/dev/null || true
  brew install "${BREW_TAP}/${BREW_FORMULA}" || brew install "$BREW_FORMULA" || return 1
  ASTRID="$(command -v astrid)" || return 1
  ok "installed via Homebrew: $ASTRID"
  return 0
}

# True when this astrid binary looks Homebrew-managed.
is_brew_astrid() {
  case "${1:-}" in
    */Cellar/astrid/*|*/homebrew/*|*/linuxbrew/*) return 0 ;;
    *) return 1 ;;
  esac
}

# Prefer Astrid's own update path when a binary already exists.
# `astrid update -y` knows brew vs cargo vs ~/.astrid/bin, swaps the CLI
# correctly, then syncs the *default* principal's distro/capsules (idempotent
# via Distro.lock). Host principals are handled separately by wire_host → init.
refresh_existing_astrid() {
  if [ -n "$BIN_ROOT" ] || [ -n "${ASTRID_BIN:-}" ]; then
    return 0
  fi

  if has_pair "$ASTRID_MANAGED_BIN"; then
    export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
    ASTRID="$ASTRID_MANAGED_BIN/astrid"
    export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"
  elif have_cmd astrid; then
    ASTRID="$(command -v astrid)"
  else
    return 1
  fi

  header "Upgrade Astrid (astrid update)"
  info "using Astrid's own updater (not a raw reinstall)…"
  if "$ASTRID" update -y 2>&1; then
    # Re-resolve after swap (managed path may have a new binary).
    if has_pair "$ASTRID_MANAGED_BIN"; then
      export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
      ASTRID="$ASTRID_MANAGED_BIN/astrid"
      export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"
    elif have_cmd astrid; then
      ASTRID="$(command -v astrid)"
    fi
    ok "astrid update: $(astrid_version)"
  else
    warn "astrid update reported an error — continuing with $(astrid_version)"
    # Managed fallback only if self-update failed (e.g. very old binary).
    if has_pair "$ASTRID_MANAGED_BIN" && ! is_brew_astrid "$ASTRID"; then
      warn "falling back to re-fetch of managed release tarball…"
      install_astrid_from_github || true
      if has_pair "$ASTRID_MANAGED_BIN"; then
        export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
        ASTRID="$ASTRID_MANAGED_BIN/astrid"
        export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"
      fi
    fi
  fi
  return 0
}

resolve_astrid() {
  if [ -n "$BIN_ROOT" ]; then
    has_pair "$BIN_ROOT" || die "--bin-root missing astrid + astrid-daemon: $BIN_ROOT"
    ASTRID="$BIN_ROOT/astrid"
    export ASTRID_BIN_ROOT="$BIN_ROOT"
    ok "using Astrid from $BIN_ROOT"
    return 0
  fi

  if [ -n "${ASTRID_BIN:-}" ] && [ -x "$ASTRID_BIN" ]; then
    ASTRID="$ASTRID_BIN"
    ok "using ASTRID_BIN=$ASTRID"
    return 0
  fi

  # Existing install → upgrade path (same one command as first install)
  if refresh_existing_astrid; then
    return 0
  fi

  # Dev trees near cwd (never auto-upgrade these)
  dir="$(pwd -P 2>/dev/null || pwd)"
  while [ "$dir" != "/" ] && [ -n "$dir" ]; do
    for sub in core/target/debug core/target/release target/debug target/release; do
      if has_pair "$dir/$sub"; then
        ASTRID="$dir/$sub/astrid"
        export ASTRID_BIN_ROOT="$dir/$sub"
        ok "found dev Astrid at $ASTRID_BIN_ROOT"
        return 0
      fi
    done
    dir="$(dirname "$dir")"
  done

  # Fresh install: GitHub Releases (macOS + Linux; matches astrid update)
  header "Install Astrid from GitHub Releases"
  if install_astrid_from_github; then
    return 0
  fi

  # Optional last resort: Homebrew (mostly macOS)
  if [ "$NO_BREW" -eq 0 ] && install_astrid_from_brew; then
    return 0
  fi

  die "could not install Astrid.
Tried: GitHub releases (${ASTRID_RELEASE_REPO}) then Homebrew.
Manual: https://github.com/${ASTRID_RELEASE_REPO}/releases
  or: brew install ${BREW_TAP}/${BREW_FORMULA}"
}

astrid_version() {
  "$ASTRID" --version 2>/dev/null | head -n1 || printf 'unknown'
}


# ---------------------------------------------------------------------------
# Base Astrid (complete product without any host plugin)
# ---------------------------------------------------------------------------
base_already_initialized() {
  # Heuristic: default principal home or local capsules dir exists.
  home="${ASTRID_HOME:-$HOME/.astrid}"
  [ -d "$home/home/default" ] || [ -d "$home/home" ] || [ -f "$home/config.toml" ]
}

ensure_base_astrid() {
  header "Base Astrid"
  if [ "$SKIP_INIT" -eq 1 ]; then
    warn "skipping base init (--skip-init)"
    return 0
  fi
  if base_already_initialized && [ "${UPGRADE:-0}" -eq 0 ]; then
    ok "runtime home present (${ASTRID_HOME:-$HOME/.astrid})"
    return 0
  fi
  if [ "${UPGRADE:-0}" -eq 1 ]; then
    info "re-applying base init (--upgrade)..."
  else
    info "initializing default principal (base install)..."
  fi
  if "$ASTRID" init -y 2>&1; then
    ok "base Astrid initialized"
  else
    warn "astrid init -y reported an error — run: astrid doctor"
  fi
}

# ---------------------------------------------------------------------------
# Host plugins (optional)
# ---------------------------------------------------------------------------
# Prefer local checkout distros when this script lives in a clone.
script_dir() {
  # When piped to sh, $0 is often "sh" — no local tree.
  case "$0" in
    /*|//*) dirname "$0" ;;
    ./*|../*|*/*) CDPATH= cd -- "$(dirname "$0")" 2>/dev/null && pwd -P ;;
    *) printf '' ;;
  esac
}

distro_url() {
  host="$1"
  local_root="$(script_dir)"
  if [ -n "$local_root" ] && [ -f "$local_root/distros/${host}.toml" ]; then
    printf '%s\n' "$local_root/distros/${host}.toml"
    return 0
  fi
  printf '%s/%s.toml\n' "$DISTRO_BASE" "$host"
}

principal_for() {
  case "$1" in
    claude) printf 'claude-code\n' ;;
    grok)   printf 'grok-code\n' ;;
    codex)  printf 'codex-code\n' ;;
    *)      die "principal_for: bad host $1" ;;
  esac
}

pretty_host() {
  case "$1" in
    claude) printf 'Claude Code\n' ;;
    grok)   printf 'Grok Build\n' ;;
    codex)  printf 'Codex\n' ;;
  esac
}

# True if this principal already has a Distro.lock (init would short-circuit).
principal_has_lock() {
  p="$1"
  home="${ASTRID_HOME:-$HOME/.astrid}"
  [ -f "$home/home/${p}/.config/distro.lock" ] || [ -f "$home/home/${p}/.config/Distro.lock" ]
}

# Set once: seed default principal at most once per run (daemon uplink).
DEFAULT_SEEDED=0

wire_host() {
  host="$1"
  principal="$(principal_for "$host")"
  distro="$(distro_url "$host")"
  pretty="$(pretty_host "$host")"

  header "Wire $pretty → principal $principal"

  if [ "$SKIP_INIT" -eq 1 ]; then
    warn "skipping astrid init (--skip-init)"
    return 0
  fi

  info "distro: $distro"

  # Seed default once if it has no lock yet. astrid init is idempotent when
  # Distro.lock id+version matches the remote manifest ("already installed").
  if [ "$DEFAULT_SEEDED" -eq 0 ]; then
    if principal_has_lock default; then
      info "default principal already has Distro.lock — skip re-seed"
    else
      info "provisioning default principal (daemon uplink, first time)..."
      if ! "$ASTRID" init --distro "$distro" -y 2>&1; then
        warn "init for default failed — continuing with principal $principal"
      fi
    fi
    DEFAULT_SEEDED=1
  fi

  # Host principal: astrid init does the right thing —
  #   * no lock / stale distro version → install/upgrade capsules
  #   * Distro.lock fresh              → no-op ("already installed")
  # Do NOT raw-reinstall or agent-modify as a substitute for init.
  if principal_has_lock "$principal"; then
    info "ensuring ${principal} is current (init is a no-op when Distro.lock is fresh)..."
  else
    info "first-time provision for ${principal}..."
  fi
  if ! "$ASTRID" init --distro "$distro" --principal "$principal" -y 2>&1; then
    die "astrid init failed for $principal
Capsule installs hit GitHub Releases over the API (60 req/hr without a token).
Export a token and re-run:
  export GH_TOKEN=\$(gh auth token)   # if you use gh
  # or: export GITHUB_TOKEN=ghp_...
  curl -fsSL https://astridos.org/install.sh | sh"
  fi

  # Best-effort: ensure the agent profile may *use* the distro capsules.
  # This is a grant list, not an install — init already wrote the files.
  case "$host" in
    claude)
      grants="claude-runner astrid-mcp claude-install astrid-capsule-cli astrid-capsule-forge astrid-capsule-fs astrid-capsule-http astrid-capsule-shell astrid-capsule-skills astrid-capsule-system"
      ;;
    codex)
      grants="codex-runner astrid-mcp codex-install astrid-capsule-cli astrid-capsule-forge astrid-capsule-fs astrid-capsule-http astrid-capsule-shell astrid-capsule-skills astrid-capsule-system"
      ;;
    grok)
      grants="astrid-mcp astrid-capsule-cli astrid-capsule-forge astrid-capsule-fs astrid-capsule-http astrid-capsule-shell astrid-capsule-skills astrid-capsule-system"
      ;;
  esac
  add_args=""
  for c in $grants; do
    add_args="$add_args --add-capsule $c"
  done
  # shellcheck disable=SC2086
  if "$ASTRID" agent modify "$principal" $add_args 2>/dev/null; then
    ok "ensured agent grants for $principal"
  else
    warn "could not update agent grants (ok if astrid agent modify is unavailable)"
  fi

  ok "$pretty ready under principal ${C_BOLD}$principal${C_RESET}"
}

plugin_hint() {
  host="$1"
  case "$host" in
    claude)
      say "  ${C_DIM}Plugin:${C_RESET} claude plugin marketplace add ${ORACLES_REPO}"
      say "           then enable the ${C_BOLD}astrid${C_RESET} plugin (Claude Code)"
      ;;
    grok)
      say "  ${C_DIM}Plugin:${C_RESET} grok plugin install <path-to-oracles>/plugins/grok --trust"
      say "           then enable the ${C_BOLD}astrid${C_RESET} plugin (Grok Build)"
      ;;
    codex)
      say "  ${C_DIM}Plugin:${C_RESET} install plugins/codex from ${ORACLES_REPO}"
      say "           enable the ${C_BOLD}astrid${C_RESET} plugin (Codex · .agents/plugins)"
      ;;
  esac
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
# Capsule installs call api.github.com (60/hr anonymous). Prefer an existing
# GH_TOKEN/GITHUB_TOKEN; else lift one from `gh auth token` when available.
ensure_github_token() {
  if [ -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then
    info "GitHub API: using GH_TOKEN/GITHUB_TOKEN from environment"
    return 0
  fi
  if have_cmd gh; then
    _tok="$(gh auth token 2>/dev/null || true)"
    if [ -n "$_tok" ]; then
      export GH_TOKEN="$_tok"
      ok "GitHub API: using token from gh auth (avoids 60/hr anonymous limit)"
      return 0
    fi
  fi
  warn "no GH_TOKEN/GITHUB_TOKEN — capsule installs use anonymous GitHub API (60/hr)."
  warn "Multi-host setups can exhaust that mid-run. Fix: export GH_TOKEN=\$(gh auth token)"
}

main() {
  say ""
  say "${C_BOLD}Astrid${C_RESET}"
  say "${C_DIM}secure OS for AI agents · optional plugins for Claude · Grok · Codex${C_RESET}"
  say ""

  resolve_astrid
  info "version: $(astrid_version)"
  ensure_github_token
  ensure_base_astrid

  # Build host list
  hosts=""
  if [ "$ALL_HOSTS" -eq 1 ]; then
    hosts="claude grok codex"
    info "mode: --all (every host)"
  elif [ -n "$REQUESTED_HOSTS" ]; then
    hosts="$REQUESTED_HOSTS"
    info "mode: explicit --host"
  else
    detected=""
    detect_claude && detected="${detected} claude"
    detect_grok   && detected="${detected} grok"
    detect_codex  && detected="${detected} codex"
    hosts="$detected"
    if [ -n "$hosts" ]; then
      info "detected hosts: $(printf '%s' "$hosts" | sed 's/^ *//;s/  */, /g')"
    else
      info "no coding-host plugin detected (base Astrid is enough to start)"
    fi
  fi

  if [ "$BASE_ONLY" -eq 1 ]; then
    hosts=""
    info "mode: --base-only (skip host plugins)"
  fi

  # Interactive confirm when multiple detected and TTY
  if [ "$YES" -eq 0 ] && [ -t 0 ] && [ -n "$hosts" ] && [ "$ALL_HOSTS" -eq 0 ] && [ -z "$REQUESTED_HOSTS" ]; then
    count=0
    for _ in $hosts; do count=$((count + 1)); done
    if [ "$count" -gt 1 ]; then
      say ""
      printf "Wire all detected hosts? [Y/n] "
      read -r ans || ans=Y
      case "$ans" in
        n|N|no|No) 
          say "Pick hosts with: install.sh --host claude"
          exit 0
          ;;
      esac
    fi
  fi

  if [ -z "$hosts" ]; then
    header "Base Astrid ready"
    ok "You have a full Astrid install — no host plugin required"
    say ""
    say "Try:"
    say "  ${C_BOLD}astrid doctor${C_RESET}     health check"
    say "  ${C_BOLD}astrid chat${C_RESET}       session (if your distro includes a model)"
    say ""
    say "Optional — install a host plugin when you use one:"
    say "  curl -fsSL https://astridos.org/install.sh | sh -s -- --host claude"
    say "  ${C_DIM}(or re-run this installer after installing Claude / Grok / Codex)${C_RESET}"
    say ""
    exit 0
  fi

  for h in $hosts; do
    wire_host "$h"
  done

  header "Done"
  ok "Base Astrid + host plugin(s)"
  say "Shared tool backend: ${C_BOLD}astrid-mcp${C_RESET}"
  say "Plugins installed:"
  for h in $hosts; do
    say "  • $(pretty_host "$h")  →  $(principal_for "$h")"
    plugin_hint "$h"
  done
  say ""
  say "Verify:  ${C_BOLD}astrid doctor${C_RESET}"
  say "Docs:    https://github.com/${ORACLES_REPO}"
  say ""
  say "${C_DIM}Re-run the same command anytime to upgrade + pick up new hosts:${C_RESET}"
  say "  curl -fsSL https://astridos.org/install.sh | sh"
  say ""
}

main
