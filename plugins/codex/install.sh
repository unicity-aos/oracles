#!/bin/sh
set -eu

plugin_root() {
  script_dir="$(dirname "$0")"
  (cd "$script_dir" >/dev/null 2>&1 && pwd -P) || printf '%s\n' "$script_dir"
}

usage() {
  cat <<'EOF'
usage: ./install.sh [--bin-root PATH] [--skip-codex-install]

Installs the Astrid Codex plugin against a matched Astrid CLI/daemon pair.

Resolution order:
  1. --bin-root PATH or ASTRID_BIN_ROOT
  2. ASTRID_BIN + ASTRID_DAEMON
  3. nearby dev checkout target/debug or target/release
  4. cargo/local/Homebrew/PATH installs

When a bin root is selected, .mcp.json is written with:
  ASTRID_BIN_ROOT=<resolved root>
EOF
}

bin_root=""
skip_codex_install=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --bin-root)
      shift
      bin_root="${1:-}"
      ;;
    --skip-codex-install)
      skip_codex_install=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "install.sh: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

has_pair() {
  root="$1"
  [ -x "$root/astrid" ] && [ -x "$root/astrid-daemon" ]
}

dev_bin_roots() {
  root="$(plugin_root)"
  for base in "$(pwd -P 2>/dev/null || pwd)" "$root"; do
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

resolve_bin_root() {
  if [ -n "$bin_root" ]; then
    if has_pair "$bin_root"; then
      printf '%s\n' "$bin_root"
      return 0
    fi
    echo "install.sh: --bin-root does not contain executable astrid and astrid-daemon: $bin_root" >&2
    return 1
  fi

  if [ -n "${ASTRID_BIN_ROOT:-}" ]; then
    if has_pair "$ASTRID_BIN_ROOT"; then
      printf '%s\n' "$ASTRID_BIN_ROOT"
      return 0
    fi
    echo "install.sh: ASTRID_BIN_ROOT does not contain executable astrid and astrid-daemon: $ASTRID_BIN_ROOT" >&2
    return 1
  fi

  if [ -n "${ASTRID_BIN:-}" ] || [ -n "${ASTRID_DAEMON:-}" ]; then
    if [ -x "${ASTRID_BIN:-}" ] && [ -x "${ASTRID_DAEMON:-}" ]; then
      dirname "$ASTRID_BIN"
      return 0
    fi
    echo "install.sh: ASTRID_BIN and ASTRID_DAEMON must both be executable when either is set" >&2
    return 1
  fi

  for root in $(dev_bin_roots); do
    if has_pair "$root"; then
      printf '%s\n' "$root"
      return 0
    fi
  done

  for root in \
    "${CARGO_HOME:-$HOME/.cargo}/bin" \
    "$HOME/.cargo/bin" \
    "$HOME/.astrid/bin" \
    /opt/homebrew/bin \
    /usr/local/bin; do
    if has_pair "$root"; then
      printf '%s\n' "$root"
      return 0
    fi
  done

  if command -v astrid >/dev/null 2>&1; then
    cli="$(command -v astrid)"
    root="$(dirname "$cli")"
    if has_pair "$root"; then
      printf '%s\n' "$root"
      return 0
    fi
  fi

  echo "install.sh: could not find Astrid. Build it, install it, or rerun with --bin-root PATH." >&2
  return 1
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

write_mcp_config() {
  root="$1"
  plugin="$(plugin_root)"
  escaped_plugin="$(json_escape "$plugin")"
  escaped_root="$(json_escape "$root")"
  tmp="$plugin/.mcp.json.tmp.$$"
  cat >"$tmp" <<EOF
{
  "mcpServers": {
    "astrid": {
      "command": "./bin/astrid-up",
      "args": ["--principal", "codex-code"],
      "cwd": "$escaped_plugin",
      "env": {
        "ASTRID_BIN_ROOT": "$escaped_root"
      }
    }
  }
}
EOF
  mv "$tmp" "$plugin/.mcp.json"
}

root="$(resolve_bin_root)"
write_mcp_config "$root"

echo "Astrid configured with ASTRID_BIN_ROOT=$root"

if [ "$skip_codex_install" = "0" ]; then
  if ! command -v codex >/dev/null 2>&1; then
    echo "install.sh: codex CLI not found; wrote .mcp.json but did not install the plugin" >&2
    exit 0
  fi
  codex plugin marketplace add unicity-astrid/oracles || true
  codex plugin marketplace upgrade astrid-oracles || true
  codex plugin add astrid@astrid-oracles
fi
