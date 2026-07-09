# Shared Astrid binary resolution for the Codex plugin scripts.
#
# The Codex marketplace packages only this plugin directory. Keep this file as
# a regular file here rather than a symlink into plugins/common.

# shellcheck shell=sh

_astrid_resolve_plugin_root() {
  if [ -n "${ASTRID_PLUGIN_ROOT:-}" ]; then
    printf '%s\n' "$ASTRID_PLUGIN_ROOT"
    return 0
  fi
  _here="$(CDPATH= cd -- "$(dirname "$0")" 2>/dev/null && pwd -P)" || _here="$(dirname "$0")"
  case "$_here" in
    */bin) printf '%s\n' "$(dirname "$_here")" ;;
    *)
      _script="${0:-}"
      case "$_script" in
        */bin/*|bin/*)
          _bin="$(CDPATH= cd -- "$(dirname "$_script")" 2>/dev/null && pwd -P)" || _bin="$(dirname "$_script")"
          printf '%s\n' "$(dirname "$_bin")"
          ;;
        *) printf '%s\n' "$_here" ;;
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
# Source priority: direct paths, configured root, plugin config, the recorded
# daemon pair, local builds, Cargo/manual installs, Homebrew, then PATH.
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

  # Reuse the CLI/daemon pair recorded by Astrid before selecting a nearby
  # worktree, unless the operator already supplied an explicit override.
  if pair="$(_astrid_running_daemon_pair)"; then
    printf 'running|%s\n' "$pair"
    return 0
  fi

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

astrid_resolve_apply() {
  _pair="$(astrid_resolve_pair)" || return $?
  ASTRID_SOURCE="${_pair%%|*}"
  _rest="${_pair#*|}"
  ASTRID="${_rest%%|*}"
  ASTRID_DAEMON="${_rest#*|}"
  export ASTRID ASTRID_DAEMON ASTRID_SOURCE
  return 0
}
