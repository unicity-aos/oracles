# Unicity AOS CLI resolution for the Codex plugin scripts.
#
# The public `aos` command owns runtime-home selection, quiet engine startup,
# and authenticated command dispatch. Plugin scripts never launch or inspect
# the engine daemon directly.

# shellcheck shell=sh

# Run every product integration against one AOS-owned workspace. Astrid binds
# its daemon to a workspace selection, while Codex hooks and MCP processes may
# start from different directories. The host project remains event context.
aos_enter_product_workspace() {
  workspace="${AOS_HOME:-$HOME/.aos}/runtime"
  [ ! -L "$workspace" ] || {
    echo "aos-workspace: refusing symlinked product runtime: $workspace" >&2
    return 1
  }
  mkdir -p "$workspace" || return 1
  chmod 700 "$workspace" || return 1
  CDPATH= cd -P -- "$workspace" || return 1
}

_aos_resolve_plugin_root() {
  if [ -n "${AOS_PLUGIN_ROOT:-}" ]; then
    printf '%s\n' "$AOS_PLUGIN_ROOT"
    return 0
  fi
  if [ -n "${CODEX_PLUGIN_ROOT:-${PLUGIN_ROOT:-}}" ]; then
    printf '%s\n' "${CODEX_PLUGIN_ROOT:-$PLUGIN_ROOT}"
    return 0
  fi
  _here="$(CDPATH= cd -- "$(dirname "$0")" 2>/dev/null && pwd -P)" || _here="$(dirname "$0")"
  case "$_here" in
    */bin) printf '%s\n' "$(dirname "$_here")" ;;
    *) printf '%s\n' "$_here" ;;
  esac
}

_aos_mcp_config_bin() {
  plugin_root="$1"
  config="$plugin_root/.mcp.json"
  [ -f "$config" ] || return 1
  sed -n 's/.*"AOS_BIN"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$config" | sed -n '1p'
}

# Print: <source>|<aos-cli-path>
aos_resolve_cli() {
  plugin_root="$(_aos_resolve_plugin_root)"

  if [ -n "${AOS_BIN:-}" ]; then
    if [ -x "$AOS_BIN" ]; then
      printf 'direct|%s\n' "$AOS_BIN"
      return 0
    fi
    echo "aos-resolve: AOS_BIN does not point to an executable file" >&2
    return 127
  fi

  if [ -n "${AOS_BIN_ROOT:-}" ]; then
    if [ -x "$AOS_BIN_ROOT/aos" ]; then
      printf 'bin-root|%s/aos\n' "$AOS_BIN_ROOT"
      return 0
    fi
    echo "aos-resolve: AOS_BIN_ROOT does not contain an executable aos binary:" >&2
    echo "aos-resolve:   $AOS_BIN_ROOT" >&2
    return 127
  fi

  config_bin="$(_aos_mcp_config_bin "$plugin_root" || true)"
  if [ -n "$config_bin" ]; then
    if [ -x "$config_bin" ]; then
      printf 'mcp-config|%s\n' "$config_bin"
      return 0
    fi
    echo "aos-resolve: .mcp.json AOS_BIN is not executable: $config_bin" >&2
    return 127
  fi

  if [ -n "${AOS_HOME:-}" ]; then
    if [ -x "$AOS_HOME/bin/aos" ]; then
      printf 'home|%s/bin/aos\n' "$AOS_HOME"
      return 0
    fi
    echo "aos-resolve: AOS_HOME is set but does not contain bin/aos: $AOS_HOME" >&2
    return 127
  fi

  managed="$HOME/.aos/bin/aos"
  if [ -x "$managed" ]; then
    printf 'managed|%s\n' "$managed"
    return 0
  fi

  for root in \
    "${CARGO_HOME:-$HOME/.cargo}/bin" \
    "$HOME/.cargo/bin" \
    /opt/homebrew/bin \
    /usr/local/bin
  do
    if [ -x "$root/aos" ]; then
      printf 'installed|%s/aos\n' "$root"
      return 0
    fi
  done

  if command -v aos >/dev/null 2>&1; then
    printf 'path|%s\n' "$(command -v aos)"
    return 0
  fi

  return 127
}

aos_resolve_apply() {
  _resolved="$(aos_resolve_cli)" || return $?
  AOS_SOURCE="${_resolved%%|*}"
  AOS="${_resolved#*|}"
  export AOS AOS_SOURCE
  return 0
}
