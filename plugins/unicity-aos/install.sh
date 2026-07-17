#!/bin/sh
set -eu

plugin_root() {
  script_dir="$(dirname "$0")"
  (cd "$script_dir" >/dev/null 2>&1 && pwd -P) || printf '%s\n' "$script_dir"
}

usage() {
  cat <<'EOF'
usage: ./install.sh [--bin-root PATH] [--skip-codex-install]

Configures the Unicity AOS Codex plugin against an installed AOS CLI.

Resolution order:
  1. --bin-root PATH or AOS_BIN_ROOT
  2. AOS_BIN
  3. explicit AOS_HOME
  4. ~/.aos/bin/aos, Homebrew, or PATH
EOF
}

bin_root=""
skip_codex_install=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --bin-root)
      shift
      bin_root="${1:-}"
      [ -n "$bin_root" ] || { echo "install.sh: --bin-root requires a path" >&2; exit 2; }
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

root="$(plugin_root)"
export AOS_PLUGIN_ROOT="$root"
if [ -n "$bin_root" ]; then
  export AOS_BIN_ROOT="$bin_root"
fi

# shellcheck source=bin/lib-aos-resolve.sh
. "$root/bin/lib-aos-resolve.sh"
if ! aos_resolve_apply; then
  echo "install.sh: could not find Unicity AOS" >&2
  echo "install.sh: install it with https://aos.unicity.ai/install.sh or pass --bin-root PATH" >&2
  exit 1
fi

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

write_mcp_config() {
  plugin="$1"
  escaped_aos="$(json_escape "$AOS")"
  tmp="$plugin/.mcp.json.tmp.$$"
  cat >"$tmp" <<EOF
{
  "mcpServers": {
    "aos": {
      "command": "/bin/sh",
      "args": [
        "./bin/aos-up",
        "--principal",
        "codex-code"
      ],
      "cwd": ".",
      "startup_timeout_sec": 305,
      "env": {
        "AOS_BIN": "$escaped_aos"
      }
    }
  }
}
EOF
  mv "$tmp" "$plugin/.mcp.json"
}

write_mcp_config "$root"
echo "Unicity AOS configured with $AOS"

if [ "$skip_codex_install" = "0" ]; then
  if ! command -v codex >/dev/null 2>&1; then
    echo "install.sh: Codex CLI not found; wrote .mcp.json but did not install the plugin" >&2
    exit 0
  fi

  if codex plugin marketplace list 2>/dev/null \
    | awk '$1 == "unicity-aos-oracles" { found = 1 } END { exit !found }'
  then
    codex plugin marketplace upgrade unicity-aos-oracles
  else
    codex plugin marketplace add unicity-aos/oracles
  fi
  codex plugin add unicity-aos@unicity-aos-oracles

fi
