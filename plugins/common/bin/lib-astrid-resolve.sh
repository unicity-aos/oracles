# Shared Astrid binary resolution for the Claude Code plugin scripts.
#
# Sourced by astrid-up, astrid-doctor, and the HUD status lines so every surface
# picks the same CLI/daemon pair. Mirrors the Codex plugin's resolution
# order so a nearby worktree debug build wins over a stale Homebrew install.
#
# Resolution order (first matching executable pair wins):
#   1. ASTRID_BIN + ASTRID_DAEMON (both must be executable)
#   2. ASTRID_BIN_ROOT env (must contain astrid + astrid-daemon)
#   3. ASTRID_BIN_ROOT from this plugin's .mcp.json env block
#   4. The CLI/daemon pair recorded by the Astrid run marker
#   5. Nearby dev checkout: walk up from cwd and the plugin root looking for
#      core/target/{debug,release} or target/{debug,release}
#   6. Installed locations: $CARGO_HOME/bin, ~/.cargo/bin, ~/.astrid/bin,
#      Homebrew, /usr/local/bin
#   7. PATH (`command -v astrid`) with sibling astrid-daemon
#
# Exports via stdout (for capture) or sets ASTRID / ASTRID_DAEMON / ASTRID_SOURCE
# when astrid_resolve_apply is called.
#
# Requires: caller has set ASTRID_PLUGIN_ROOT (absolute plugin root) before
# sourcing, OR this file lives at <plugin>/bin/lib-astrid-resolve.sh so the
# default root is dirname of this file's parent.

# shellcheck shell=sh

_astrid_resolve_plugin_root() {
  if [ -n "${ASTRID_PLUGIN_ROOT:-}" ]; then
    printf '%s\n' "$ASTRID_PLUGIN_ROOT"
    return 0
  fi
  # This file is bin/lib-astrid-resolve.sh → plugin root is ..
  _here="$(CDPATH= cd -- "$(dirname "$0")" 2>/dev/null && pwd -P)" || _here="$(dirname "$0")"
  # When sourced, $0 is the caller's path (bin/astrid-up), so dirname is bin/.
  # Prefer the caller's directory parent when this looks like a bin/ script.
  case "$_here" in
    */bin) printf '%s\n' "$(dirname "$_here")" ;;
    *)
      # Sourcing via `. "$root/bin/lib-astrid-resolve.sh"` leaves $0 as the
      # caller; walk from BASH_SOURCE/ZSH equivalent is unavailable in POSIX.
      # Callers should set ASTRID_PLUGIN_ROOT. Fallback: dirname of $0's parent
      # if $0 ends in a bin script name.
      _script="${0:-}"
      case "$_script" in
        */bin/*|bin/*)
          _bin="$(CDPATH= cd -- "$(dirname "$_script")" 2>/dev/null && pwd -P)" || _bin="$(dirname "$_script")"
          printf '%s\n' "$(dirname "$_bin")"
          ;;
        *)
          printf '%s\n' "$_here"
          ;;
      esac
      ;;
  esac
}

_astrid_bin_pair_from_root() {
  root="$1"
  [ -x "$root/astrid" ] && [ -x "$root/astrid-daemon" ] || return 1
  printf '%s|%s\n' "$root/astrid" "$root/astrid-daemon"
}

_astrid_mcp_config_bin_root() {
  plugin_root="$1"
  config="$plugin_root/.mcp.json"
  [ -f "$config" ] || return 1
  # Prefer an env block entry; tolerate either spacing style.
  sed -n 's/.*"ASTRID_BIN_ROOT"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$config" | sed -n '1p'
}

_astrid_running_daemon_pair() {
  astrid_home="${ASTRID_HOME:-$HOME/.astrid}"
  pid_file="$astrid_home/run/system.pid"
  [ -f "$pid_file" ] || return 1

  pid="$(sed -n '1p' "$pid_file" 2>/dev/null | sed 's/[^0-9].*$//')"
  daemon="$(sed -n '2p' "$pid_file" 2>/dev/null)"
  [ -n "$pid" ] && [ -x "$daemon" ] || return 1
  _astrid_bin_pair_from_root "$(dirname "$daemon")"
}

_astrid_dev_bin_roots() {
  plugin_root="$1"
  for base in "$(pwd -P 2>/dev/null || pwd)" "$plugin_root"; do
    dir="$base"
    while [ "$dir" != "/" ] && [ -n "$dir" ]; do
      printf '%s\n' "$dir/core/target/debug"
      printf '%s\n' "$dir/core/target/release"
      printf '%s\n' "$dir/target/debug"
      printf '%s\n' "$dir/target/release"
      dir="$(dirname "$dir")"
    done
  done
}

# Print: <source>|<cli-path>|<daemon-path>
# source is one of: direct|bin-root|mcp-config|running|dev|installed|path
# Exit 127 when nothing is found (no stdout).
astrid_resolve_pair() {
  plugin_root="$(_astrid_resolve_plugin_root)"

  if [ -n "${ASTRID_BIN:-}" ] || [ -n "${ASTRID_DAEMON:-}" ]; then
    if [ -x "${ASTRID_BIN:-}" ] && [ -x "${ASTRID_DAEMON:-}" ]; then
      printf 'direct|%s|%s\n' "$ASTRID_BIN" "$ASTRID_DAEMON"
      return 0
    fi
    echo "astrid-resolve: ASTRID_BIN and ASTRID_DAEMON must both point to executable files" >&2
    return 127
  fi

  if [ -n "${ASTRID_BIN_ROOT:-}" ]; then
    pair="$(_astrid_bin_pair_from_root "$ASTRID_BIN_ROOT")" || {
      echo "astrid-resolve: ASTRID_BIN_ROOT is set but does not contain executable astrid and astrid-daemon:" >&2
      echo "astrid-resolve:   $ASTRID_BIN_ROOT" >&2
      return 127
    }
    printf 'bin-root|%s\n' "$pair"
    return 0
  fi

  config_root="$(_astrid_mcp_config_bin_root "$plugin_root" || true)"
  if [ -n "$config_root" ]; then
    pair="$(_astrid_bin_pair_from_root "$config_root")" || {
      echo "astrid-resolve: .mcp.json ASTRID_BIN_ROOT does not contain executable astrid and astrid-daemon:" >&2
      echo "astrid-resolve:   $config_root" >&2
      return 127
    }
    printf 'mcp-config|%s\n' "$pair"
    return 0
  fi

  # Astrid pins its own CLI/daemon pair in system.pid. Reuse that pair before
  # choosing a nearby worktree so host plugins reconnect rather than reject a
  # daemon started from another checkout. astrid-up validates stale markers.
  if pair="$(_astrid_running_daemon_pair)"; then
    printf 'running|%s\n' "$pair"
    return 0
  fi

  # Word-splitting of newlines is intentional here — roots have no spaces.
  # shellcheck disable=SC2046
  for root in $(_astrid_dev_bin_roots "$plugin_root"); do
    if pair="$(_astrid_bin_pair_from_root "$root")"; then
      printf 'dev|%s\n' "$pair"
      return 0
    fi
  done

  for root in \
    "${CARGO_HOME:-$HOME/.cargo}/bin" \
    "$HOME/.cargo/bin" \
    "$HOME/.astrid/bin" \
    /opt/homebrew/bin \
    /usr/local/bin; do
    if pair="$(_astrid_bin_pair_from_root "$root")"; then
      printf 'installed|%s\n' "$pair"
      return 0
    fi
  done

  if command -v astrid >/dev/null 2>&1; then
    cli="$(command -v astrid)"
    daemon="$(dirname "$cli")/astrid-daemon"
    if [ -x "$daemon" ]; then
      printf 'path|%s|%s\n' "$cli" "$daemon"
      return 0
    fi
  fi

  return 127
}

# Set ASTRID, ASTRID_DAEMON, ASTRID_SOURCE in the caller's shell.
# Returns 0 on success, 127 on failure (leaves vars unset/stale).
astrid_resolve_apply() {
  _pair="$(astrid_resolve_pair)" || return $?
  ASTRID_SOURCE="${_pair%%|*}"
  _rest="${_pair#*|}"
  ASTRID="${_rest%%|*}"
  ASTRID_DAEMON="${_rest#*|}"
  export ASTRID ASTRID_DAEMON ASTRID_SOURCE
  return 0
}
