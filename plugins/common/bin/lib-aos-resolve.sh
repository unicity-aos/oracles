# Shared Unicity AOS command resolution for host plugins.
#
# Product plugins always enter through `aos`. The AOS command owns runtime
# selection, AOS_HOME, authenticated principal startup, and the private
# Astrid daemon. These helpers deliberately never look in ~/.astrid and never
# execute the private `astrid` command directly.

# shellcheck shell=sh

_aos_resolve_plugin_root() {
  if [ -n "${AOS_PLUGIN_ROOT:-}" ]; then
    printf '%s\n' "$AOS_PLUGIN_ROOT"
    return 0
  fi

  _here="$(CDPATH= cd -- "$(dirname "$0")" 2>/dev/null && pwd -P)" \
    || _here="$(dirname "$0")"
  case "$_here" in
    */bin) printf '%s\n' "$(dirname "$_here")" ;;
    *) printf '%s\n' "$_here" ;;
  esac
}

_aos_mcp_config_bin() {
  _plugin_root="$1"
  _config="$_plugin_root/.mcp.json"
  [ -f "$_config" ] || return 1
  sed -n 's/.*"AOS_BIN"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$_config" \
    | sed -n '1p'
}

# Print: <source>|<aos-command>
# Exit 127 when no product command is available.
aos_resolve() {
  _plugin_root="$(_aos_resolve_plugin_root)"

  if [ -n "${AOS_BIN:-}" ]; then
    if [ -x "$AOS_BIN" ]; then
      printf 'direct|%s\n' "$AOS_BIN"
      return 0
    fi
    echo "aos-resolve: AOS_BIN is set but is not executable: $AOS_BIN" >&2
    return 127
  fi

  if [ -n "${AOS_HOME:-}" ]; then
    _candidate="$AOS_HOME/bin/aos"
    if [ -x "$_candidate" ]; then
      printf 'home|%s\n' "$_candidate"
      return 0
    fi
    echo "aos-resolve: AOS_HOME is set but does not contain bin/aos: $AOS_HOME" >&2
    return 127
  fi

  _configured="$(_aos_mcp_config_bin "$_plugin_root" || true)"
  if [ -n "$_configured" ]; then
    if [ -x "$_configured" ]; then
      printf 'mcp-config|%s\n' "$_configured"
      return 0
    fi
    echo "aos-resolve: .mcp.json AOS_BIN is not executable: $_configured" >&2
    return 127
  fi

  _candidate="$HOME/.aos/bin/aos"
  if [ -x "$_candidate" ]; then
    printf 'installed|%s\n' "$_candidate"
    return 0
  fi

  if command -v aos >/dev/null 2>&1; then
    printf 'path|%s\n' "$(command -v aos)"
    return 0
  fi

  return 127
}

# Set AOS and AOS_SOURCE in the caller's shell.
aos_resolve_apply() {
  _resolved="$(aos_resolve)" || return $?
  AOS_SOURCE="${_resolved%%|*}"
  AOS="${_resolved#*|}"
  export AOS AOS_SOURCE
}
