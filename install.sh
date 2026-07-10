#!/usr/bin/env sh
# install.sh — one command. That's the product.
#
#   curl -fsSL https://astridos.org/install.sh | sh
#
# One status line per step: box spinner + elapsed bar while it runs, a
# checkmark when it lands. Does CLI + base home + detected host plugins
# (marketplace) + host capsules. Re-run upgrades everything in place.
#
# Before writing into host apps it lists what it detected and asks once —
# Enter wires all, names pick a subset, n skips. -y or no TTY never asks.
#
# Flags ride after `sh -s --`:
#
#   curl -fsSL https://astridos.org/install.sh | sh -s -- --host claude --verbose
set -eu

ORACLES_REPO="${ASTRID_ORACLES_REPO:-unicity-astrid/oracles}"
DISTRO_BASE="${ASTRID_ORACLES_DISTRO_BASE:-https://raw.githubusercontent.com/${ORACLES_REPO}/main/distros}"
ASTRID_RELEASE_REPO="${ASTRID_RELEASE_REPO:-unicity-astrid/astrid}"
ASTRID_MANAGED_BIN="${ASTRID_HOME:-$HOME/.astrid}/bin"
BREW_TAP="${ASTRID_BREW_TAP:-unicity-astrid/tap}"
BREW_FORMULA="${ASTRID_BREW_FORMULA:-astrid}"

VERBOSE=0
NO_BREW=0
BASE_ONLY=0
SKIP_INIT=0
ALL_HOSTS=0
REQUESTED_HOSTS=""
BIN_ROOT="${ASTRID_BIN_ROOT:-}"
ASTRID=""
FAILED=0
ASSUME_YES=0
SPIN_PID=""
CLEAN_TMP=""
SPIN_TTY=0
[ -t 1 ] && SPIN_TTY=1

have() { command -v "$1" >/dev/null 2>&1; }
say()  { printf '%s\n' "$*"; }

# Run a subcommand quietly; pass its output through under --verbose.
# stdin is /dev/null: subcommands are non-interactive and must never eat
# terminal input — the host-selection prompt is the only /dev/tty reader.
q() { if [ "$VERBOSE" -eq 1 ]; then "$@" </dev/null; else "$@" </dev/null >/dev/null 2>&1; fi; }

# --- status bar: box spinner + elapsed rectangle ----------------------------
spin_start() {
  if [ "$SPIN_TTY" -eq 0 ] || [ "$VERBOSE" -eq 1 ]; then return 0; fi
  _label="$1"
  (
    start="$(date +%s)"
    i=0
    while :; do
      now="$(date +%s)"
      el=$((now - start))
      filled=$((el / 3))
      [ "$filled" -gt 10 ] && filled=10
      bar=""
      j=0
      while [ "$j" -lt 10 ]; do
        if [ "$j" -lt "$filled" ]; then bar="${bar}▮"; else bar="${bar}▯"; fi
        j=$((j + 1))
      done
      case "$i" in 0) f="◰" ;; 1) f="◳" ;; 2) f="◲" ;; *) f="◱" ;; esac
      printf '\r\033[K%s %s %s %ss' "$f" "$_label" "$bar" "$el"
      i=$(((i + 1) % 4))
      sleep 0.2 2>/dev/null || sleep 1
    done
  ) &
  SPIN_PID=$!
}

spin_stop() {
  if [ -z "$SPIN_PID" ]; then return 0; fi
  kill "$SPIN_PID" 2>/dev/null || true
  wait "$SPIN_PID" 2>/dev/null || true
  SPIN_PID=""
  [ "$SPIN_TTY" -eq 1 ] && printf '\r\033[K'
  return 0
}

ok()   { spin_stop; say "✓ $*"; }
fail() { spin_stop; say "✗ $*"; FAILED=$((FAILED + 1)); }
warn() { spin_stop; say "! $*"; }
die()  { spin_stop; say "✗ $*" >&2; exit 1; }

cleanup() {
  spin_stop
  if [ -n "$CLEAN_TMP" ]; then rm -rf "$CLEAN_TMP" 2>/dev/null || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

usage() {
  cat <<'EOF'
curl -fsSL https://astridos.org/install.sh | sh

One command: CLI, base home, plugins + capsules for hosts on this machine.
Detected hosts are listed first and wired after one Enter (names pick a
subset, n skips). Re-run any time to upgrade. Flags go after `sh -s --`:

  curl -fsSL https://astridos.org/install.sh | sh -s -- --host claude --verbose

  --host NAME   only this host (claude|grok|codex), repeatable — no prompt
  --all         every host — no prompt
  --yes, -y     wire all detected hosts without asking
  --base-only   skip host plugins/capsules
  --skip-init   skip base-home init
  --no-brew     never use Homebrew
  --bin-root D  use astrid from D
  --verbose     show subcommand output (disables the status bar)
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
        *) die "unknown host '$h' (want claude|grok|codex)" ;;
      esac
      ;;
    --all) ALL_HOSTS=1 ;;
    --base-only) BASE_ONLY=1 ;;
    --skip-init) SKIP_INIT=1 ;;
    --no-brew) NO_BREW=1 ;;
    --bin-root)
      shift
      BIN_ROOT="${1:-}"
      [ -n "$BIN_ROOT" ] || die "--bin-root needs a path"
      ;;
    --verbose|-v) VERBOSE=1 ;;
    --yes|-y) ASSUME_YES=1 ;;
    --upgrade) ;; # accepted for compat
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

if [ -z "${GH_TOKEN:-}${GITHUB_TOKEN:-}" ] && have gh; then
  _tok="$(gh auth token 2>/dev/null || true)"
  [ -n "$_tok" ] && export GH_TOKEN="$_tok"
fi

has_pair() { [ -n "$1" ] && [ -x "$1/astrid" ] && [ -x "$1/astrid-daemon" ]; }

cli_version() { "$1" --version 2>/dev/null | head -n1 | awk '{ print $NF }'; }

platform_target() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  case "${os}/${arch}" in
    Darwin/arm64|Darwin/aarch64) printf 'aarch64-apple-darwin\n' ;;
    Darwin/x86_64)               printf 'x86_64-apple-darwin\n' ;;
    Linux/x86_64|Linux/amd64)    printf 'x86_64-unknown-linux-gnu\n' ;;
    Linux/aarch64|Linux/arm64)   printf 'aarch64-unknown-linux-gnu\n' ;;
    *) return 1 ;;
  esac
}

sha256_file() {
  if have sha256sum; then sha256sum "$1" | awk '{ print $1 }'
  elif have shasum; then shasum -a 256 "$1" | awk '{ print $1 }'
  else return 1
  fi
}

# Install from GitHub Releases. Never exits: returns 1 so brew can back it up.
install_from_github() {
  target="$(platform_target)" || { warn "unsupported platform $(uname -s 2>/dev/null)/$(uname -m 2>/dev/null)"; return 1; }
  have curl || { warn "curl required for GitHub releases"; return 1; }
  tmp="$(mktemp -d 2>/dev/null || mktemp -d -t astrid-install)" || { warn "mktemp failed"; return 1; }
  CLEAN_TMP="$tmp"

  auth="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  if [ -n "${ASTRID_VERSION:-}" ]; then
    tag="v${ASTRID_VERSION#v}"
  else
    api="https://api.github.com/repos/${ASTRID_RELEASE_REPO}/releases/latest"
    meta="$(curl -fsSL --max-time 30 ${auth:+-H "Authorization: Bearer ${auth}"} "$api")" \
      || { warn "could not query GitHub releases"; return 1; }
    tag="$(printf '%s' "$meta" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
    [ -n "$tag" ] || { warn "latest release has no tag"; return 1; }
  fi
  version="${tag#v}"
  asset="astrid-${version}-${target}.tar.gz"
  base="https://github.com/${ASTRID_RELEASE_REPO}/releases/download/${tag}"

  curl -fsSL --max-time 120 -o "$tmp/$asset" "${base}/${asset}" || { warn "download failed: $asset"; return 1; }
  curl -fsSL --max-time 30 -o "$tmp/SHA256SUMS.txt" "${base}/SHA256SUMS.txt" || { warn "release has no SHA256SUMS.txt"; return 1; }
  expected="$(awk -v a="$asset" '$2 == a || $2 == "./"a { print $1; exit }' "$tmp/SHA256SUMS.txt")"
  [ -n "$expected" ] || { warn "no checksum for $asset"; return 1; }
  actual="$(sha256_file "$tmp/$asset")" || { warn "need sha256sum or shasum"; return 1; }
  [ "$expected" = "$actual" ] || { warn "checksum mismatch for $asset"; return 1; }

  mkdir -p "$ASTRID_MANAGED_BIN" || { warn "cannot create $ASTRID_MANAGED_BIN"; return 1; }
  tar -xzf "$tmp/$asset" -C "$tmp" || { warn "extract failed"; return 1; }
  found=""
  for d in "$tmp"/* "$tmp"; do
    [ -d "$d" ] || continue
    if [ -x "$d/astrid" ] && [ -x "$d/astrid-daemon" ]; then found="$d"; break; fi
  done
  [ -n "$found" ] || { warn "archive missing astrid + astrid-daemon"; return 1; }
  for b in astrid astrid-daemon astrid-build astrid-emit; do
    if [ -x "$found/$b" ]; then
      cp -f "$found/$b" "$ASTRID_MANAGED_BIN/$b" || { warn "cannot write $ASTRID_MANAGED_BIN/$b"; return 1; }
      chmod 755 "$ASTRID_MANAGED_BIN/$b" 2>/dev/null || true
    fi
  done
  export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
  ASTRID="$ASTRID_MANAGED_BIN/astrid"
  export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"

  case "$(uname -s 2>/dev/null)" in Darwin) path_rc="$HOME/.zprofile" ;; *) path_rc="$HOME/.profile" ;; esac
  if ! grep -qsF "$ASTRID_MANAGED_BIN" "$path_rc"; then
    printf '\n# Astrid CLI\nexport PATH="%s:$PATH"\n' "$ASTRID_MANAGED_BIN" >> "$path_rc" 2>/dev/null || true
  fi
  rm -rf "$tmp" 2>/dev/null || true
  CLEAN_TMP=""
  return 0
}

ensure_cli() {
  spin_start "CLI"
  if [ -n "$BIN_ROOT" ]; then
    has_pair "$BIN_ROOT" || die "--bin-root missing astrid + astrid-daemon: $BIN_ROOT"
    ASTRID="$BIN_ROOT/astrid"
    export ASTRID_BIN_ROOT="$BIN_ROOT"
    ok "CLI $(cli_version "$ASTRID") (--bin-root)"
    return 0
  fi
  if [ -n "${ASTRID_BIN:-}" ]; then
    _dir="$(dirname "$ASTRID_BIN")"
    has_pair "$_dir" || die "ASTRID_BIN dir missing astrid + astrid-daemon: $_dir"
    ASTRID="$ASTRID_BIN"
    export ASTRID_BIN_ROOT="$_dir"
    ok "CLI $(cli_version "$ASTRID") (ASTRID_BIN)"
    return 0
  fi

  if has_pair "$ASTRID_MANAGED_BIN"; then
    export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
    ASTRID="$ASTRID_MANAGED_BIN/astrid"
    export ASTRID_BIN_ROOT="$ASTRID_MANAGED_BIN"
  elif have astrid; then
    ASTRID="$(command -v astrid)"
  fi

  if [ -n "$ASTRID" ] && [ -x "$ASTRID" ]; then
    before="$(cli_version "$ASTRID")"
    if q "$ASTRID" update -y; then
      if has_pair "$ASTRID_MANAGED_BIN"; then
        export PATH="${ASTRID_MANAGED_BIN}:${PATH}"
        ASTRID="$ASTRID_MANAGED_BIN/astrid"
      elif have astrid; then
        ASTRID="$(command -v astrid)"
      fi
    fi
    after="$(cli_version "$ASTRID")"
    if [ -n "$before" ] && [ -n "$after" ] && [ "$before" != "$after" ]; then
      ok "CLI ${after} (updated from ${before})"
    else
      ok "CLI ${after:-unknown} (up to date)"
    fi
    return 0
  fi

  if install_from_github; then
    ok "CLI $(cli_version "$ASTRID") (installed)"
    return 0
  fi
  if [ "$NO_BREW" -eq 0 ] && have brew; then
    spin_start "CLI (brew)"
    q brew tap "$BREW_TAP" || true
    q brew install "${BREW_TAP}/${BREW_FORMULA}" || q brew install "$BREW_FORMULA" \
      || die "Homebrew install failed"
    ASTRID="$(command -v astrid)" || die "no astrid on PATH after brew"
    ok "CLI $(cli_version "$ASTRID") (brew)"
    return 0
  fi
  die "could not install Astrid (GitHub releases failed; brew unavailable)"
}

ensure_base() {
  if [ "$SKIP_INIT" -eq 1 ]; then return 0; fi
  spin_start "base home"
  home="${ASTRID_HOME:-$HOME/.astrid}"
  if [ -d "$home/home/default" ] || [ -f "$home/config.toml" ]; then
    ok "base home"
    return 0
  fi
  if q "$ASTRID" init -y; then
    ok "base home (initialized)"
  else
    fail "base home (astrid init) — run: astrid doctor"
  fi
}

detect_hosts() {
  hosts=""
  if [ "$BASE_ONLY" -eq 1 ]; then printf '%s' ""; return 0; fi
  if [ "$ALL_HOSTS" -eq 1 ]; then printf '%s' "claude grok codex"; return 0; fi
  if [ -n "$REQUESTED_HOSTS" ]; then printf '%s' "$REQUESTED_HOSTS"; return 0; fi
  # A host counts only when its binary actually resolves — a stale dotdir
  # must not pull plugins and capsules onto a machine that can't run them.
  if have claude || [ -x "${HOME}/.claude/local/claude" ]; then hosts="${hosts} claude"; fi
  if have grok; then hosts="${hosts} grok"; fi
  if have codex; then hosts="${hosts} codex"; fi
  printf '%s' "$hosts"
}

# One Enter of consent before writing into other apps' configs (rustup-style):
# Enter wires every detected host, names pick a subset, n skips. Prompts only
# on a real terminal in pure-detection mode; flags, -y, and CI never block.
choose_hosts() {
  if [ "$#" -eq 0 ]; then printf ''; return 0; fi
  if [ "$ASSUME_YES" -eq 1 ] || [ -n "$REQUESTED_HOSTS" ] || [ "$ALL_HOSTS" -eq 1 ]; then
    printf '%s' "$*"
    return 0
  fi
  if [ "$SPIN_TTY" -eq 0 ] || [ ! -r /dev/tty ]; then printf '%s' "$*"; return 0; fi
  labels=""
  for h in "$@"; do labels="${labels}${labels:+, }$(pretty "$h")"; done
  printf '? hosts detected: %s\n  Enter = wire all · names to pick (e.g. claude codex) · n = skip: ' "$labels" > /dev/tty
  IFS= read -r ans < /dev/tty || ans=""
  case "$ans" in
    "") printf '%s' "$*" ;;
    n|N|no|NO|none) printf '' ;;
    *)
      picked=""
      # shellcheck disable=SC2086
      for tok in $ans; do
        case "$tok" in
          claude|grok|codex) picked="${picked} ${tok}" ;;
          *) printf '! ignoring unknown host: %s\n' "$tok" > /dev/tty ;;
        esac
      done
      printf '%s' "$picked"
      ;;
  esac
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

# --- plugins: detect, then update-or-install (verbs verified per host) -------

claude_bin() {
  if have claude; then command -v claude
  elif [ -x "${HOME}/.claude/local/claude" ]; then printf '%s\n' "${HOME}/.claude/local/claude"
  else printf ''
  fi
}

# Version of an exactly-matching installed claude plugin id ('' if absent).
claude_plugin_version() {
  _cbin="$1"
  "$_cbin" plugin list 2>/dev/null | awk -v id="$2" '
    f && /Version:/ { print $2; exit }
    $0 ~ (id "($|[^-A-Za-z0-9_])") { f = 1 }
  '
}

grok_plugin_installed() {
  grok plugin list 2>/dev/null | grep -F ': astrid [' | grep -qF '(astrid-oracles)'
}

codex_plugin_version() {
  codex plugin list 2>/dev/null | grep -F 'astrid@astrid-oracles' | grep -v 'not installed' \
    | grep 'installed' | head -n1 \
    | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^[0-9][0-9.]*$/) { print $i; exit } }'
}

install_plugin() {
  host="$1"
  label="$(pretty "$host")"
  spin_start "$label plugin"
  case "$host" in
    claude)
      cbin="$(claude_bin)"
      if [ -z "$cbin" ]; then fail "$label plugin (no claude CLI)"; return 1; fi
      q "$cbin" plugin marketplace add "$ORACLES_REPO" || true
      q "$cbin" plugin marketplace update astrid-oracles || true
      legacy="$(claude_plugin_version "$cbin" "astrid@astrid")"
      if [ -n "$legacy" ]; then
        q "$cbin" plugin uninstall astrid@astrid || true
      fi
      cur="$(claude_plugin_version "$cbin" "astrid@astrid-oracles")"
      if [ -n "$cur" ]; then
        q "$cbin" plugin update astrid@astrid-oracles || true
        new="$(claude_plugin_version "$cbin" "astrid@astrid-oracles")"
        if [ -n "$new" ] && [ "$new" != "$cur" ]; then
          ok "$label plugin ${new} (updated from ${cur}${legacy:+; removed legacy astrid@astrid})"
        else
          ok "$label plugin ${cur} (up to date${legacy:+; removed legacy astrid@astrid})"
        fi
      else
        if q "$cbin" plugin install astrid@astrid-oracles; then
          new="$(claude_plugin_version "$cbin" "astrid@astrid-oracles")"
          ok "$label plugin${new:+ ${new}} (installed${legacy:+; replaced legacy astrid@astrid})"
        else
          fail "$label plugin (claude plugin install astrid@astrid-oracles)"
          return 1
        fi
      fi
      ;;
    grok)
      if ! have grok; then fail "$label plugin (no grok CLI)"; return 1; fi
      q grok plugin marketplace add "$ORACLES_REPO" || true
      q grok plugin marketplace update || true
      if grok_plugin_installed; then
        gout="$(grok plugin update astrid </dev/null 2>&1)" || true
        if [ "$VERBOSE" -eq 1 ]; then say "$gout"; fi
        vers="$(printf '%s' "$gout" | sed -n 's/.*(\([^ ]*\) -> \([^)]*\)).*/\1 \2/p' | head -n1)"
        vfrom="${vers%% *}"
        vto="${vers##* }"
        if [ -n "$vers" ] && [ "$vfrom" != "$vto" ]; then
          ok "$label plugin ${vto} (updated from ${vfrom})"
        else
          ok "$label plugin${vto:+ ${vto}} (up to date)"
        fi
      else
        # grok pins marketplace plugins by repo shorthand: astrid@owner/repo
        if q grok plugin install "astrid@${ORACLES_REPO}" --trust; then
          ok "$label plugin (installed)"
        else
          fail "$label plugin (grok plugin install astrid@${ORACLES_REPO})"
          return 1
        fi
      fi
      ;;
    codex)
      if ! have codex; then fail "$label plugin (no codex CLI)"; return 1; fi
      q codex plugin marketplace add "$ORACLES_REPO" || true
      q codex plugin marketplace upgrade astrid-oracles || true
      cur="$(codex_plugin_version)"
      # `codex plugin add` is idempotent and doubles as the upgrade path.
      if q codex plugin add astrid@astrid-oracles; then
        new="$(codex_plugin_version)"
        if [ -n "$cur" ] && [ -n "$new" ] && [ "$new" != "$cur" ]; then
          ok "$label plugin ${new} (updated from ${cur})"
        elif [ -n "$cur" ]; then
          ok "$label plugin ${cur} (up to date)"
        else
          ok "$label plugin${new:+ ${new}} (installed)"
        fi
      else
        if [ -n "$cur" ]; then
          ok "$label plugin ${cur} (kept)"
        else
          fail "$label plugin (codex plugin add astrid@astrid-oracles)"
          return 1
        fi
      fi
      ;;
  esac
}

principal_has_lock() {
  _home="${ASTRID_HOME:-$HOME/.astrid}"
  [ -f "$_home/home/$1/.config/distro.lock" ] || [ -f "$_home/home/$1/.config/Distro.lock" ]
}

install_capsules() {
  host="$1"
  p="$(principal_for "$host")"
  d="$(distro_url "$host")"
  label="$(pretty "$host")"
  spin_start "$label capsules"

  # Forward-compat: a newer CLI grants the target principal access to the
  # capsules the distro installs (`astrid init --grant-capsules`). Feature-detect
  # it so older CLIs keep their exact current behaviour (empty flag = no arg).
  grant_flag=""
  if "$ASTRID" init --help 2>/dev/null | grep -q -- --grant-capsules; then
    grant_flag="--grant-capsules"
  fi

  # Seed default once if empty (daemon uplink); init is a no-op when lock fresh.
  if ! principal_has_lock default; then
    # shellcheck disable=SC2086 # grant_flag is one token or empty; must not quote
    q "$ASTRID" init --distro "$d" $grant_flag -y || true
  fi

  had_lock=0
  if principal_has_lock "$p"; then had_lock=1; fi
  # shellcheck disable=SC2086 # grant_flag is one token or empty; must not quote
  iout="$("$ASTRID" init --distro "$d" $grant_flag --principal "$p" -y </dev/null 2>&1)" && rc=0 || rc=$?
  if [ "$VERBOSE" -eq 1 ]; then say "$iout"; fi
  if [ "$rc" -eq 0 ]; then
    if [ "$had_lock" -eq 1 ]; then
      ok "$label capsules (up to date · $p)"
    else
      ok "$label capsules (installed · $p)"
    fi
  else
    if principal_has_lock "$p"; then
      ok "$label capsules (kept · $p)"
    else
      fail "$label capsules ($p) — need GH_TOKEN? re-run with --verbose"
    fi
  fi
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
    chosen="$(choose_hosts "$@")"
    # shellcheck disable=SC2086
    set -- $chosen
    if [ "$#" -eq 0 ]; then
      ok "hosts (skipped — base only)"
    else
      for h in "$@"; do
        install_plugin "$h" || true
        install_capsules "$h"
      done
    fi
  fi

  if [ "$FAILED" -gt 0 ]; then
    say "— ${FAILED} step(s) failed · retry loudly: curl -fsSL https://astridos.org/install.sh | sh -s -- --verbose"
    exit 1
  fi
}

main
