#!/bin/sh
set -eu

plugin_root() {
  script_dir="$(dirname "$0")"
  (CDPATH= cd -- "$script_dir" >/dev/null 2>&1 && pwd -P) || printf '%s\n' "$script_dir"
}

usage() {
  cat <<'EOF'
usage: ./install.sh [OPTIONS]

Installs and provisions Unicity AOS for Grok, then installs the Grok plugin.

Options:
  --skip-aos-install    Do not run the Unicity AOS installer
  --skip-grok-install   Do not install or enable the Grok plugin
  --aos-channel C       Follow the selected AOS channel
  --aos-version V       Install an exact AOS version
  --oracle-version V    Install an exact oracle-pack version
  --yes                 Accept installer prompts
  --no-migrate-prompt   Do not prompt to migrate an existing installation
  -h, --help            Show this help

The committed .mcp.json launches the public `aos` MCP server through aos-up.
It does not expose Astrid's private daemon or runtime home.
EOF
}

skip_aos_install=0
skip_grok_install=0
aos_channel=""
aos_version=""
oracle_version=""
assume_yes=0
no_migrate_prompt=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-aos-install)
      skip_aos_install=1
      ;;
    --skip-grok-install)
      skip_grok_install=1
      ;;
    --aos-channel)
      shift
      [ "$#" -gt 0 ] || { echo "install.sh: --aos-channel requires a value" >&2; exit 2; }
      aos_channel="$1"
      ;;
    --aos-version)
      shift
      [ "$#" -gt 0 ] || { echo "install.sh: --aos-version requires a value" >&2; exit 2; }
      aos_version="$1"
      ;;
    --oracle-version)
      shift
      [ "$#" -gt 0 ] || { echo "install.sh: --oracle-version requires a value" >&2; exit 2; }
      oracle_version="$1"
      ;;
    --yes)
      assume_yes=1
      ;;
    --no-migrate-prompt)
      no_migrate_prompt=1
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

if [ "$skip_aos_install" = "0" ]; then
  set -- --host grok
  [ -z "$aos_channel" ] || set -- "$@" --aos-channel "$aos_channel"
  [ -z "$aos_version" ] || set -- "$@" --aos-version "$aos_version"
  [ -z "$oracle_version" ] || set -- "$@" --oracle-version "$oracle_version"
  [ "$assume_yes" = "0" ] || set -- "$@" --yes
  [ "$no_migrate_prompt" = "0" ] || set -- "$@" --no-migrate-prompt
  "$root/bin/aos-install" "$@"
fi

if [ "$skip_grok_install" = "1" ]; then
  exit 0
fi

if ! command -v grok >/dev/null 2>&1; then
  echo "install.sh: grok CLI not found; Unicity AOS is ready, but the Grok plugin was not installed" >&2
  exit 0
fi

grok plugin install "$root" --trust
grok plugin enable unicity-aos
