#!/usr/bin/env sh
# Install signed Unicity AOS oracle packs and their host marketplace plugins.
set -eu
umask 077

ORACLES_REPO="${AOS_ORACLES_REPO:-unicity-aos/oracles}"
ORACLES_VERSION="${AOS_ORACLES_VERSION:-0.2.0}"
AOS_INSTALL_URL="${AOS_INSTALL_URL:-https://aos.unicity.ai/install.sh}"
AOS_HOME_DIR="${AOS_HOME:-$HOME/.aos}"
AOS_CHANNEL=""
AOS_VERSION=""
CLAUDE_AUTH_MODE="${AOS_CLAUDE_AUTH_MODE:-api_key}"
CLAUDE_INTERACTION_MODE="${AOS_CLAUDE_INTERACTION_MODE:-headless}"
COSIGN_VERSION=v3.1.1
ASSUME_YES=0
ALL_HOSTS=0
NO_INSTALL_AOS=0
NO_MIGRATE_PROMPT=0
OFFLINE=0
REQUESTED_HOSTS=""
LOCAL_ASSETS="${AOS_ORACLE_ASSETS:-}"
WORK=""
COSIGN=""
STARTED_DAEMON=0
RELEASE_STAGE=""
PLUGIN_SNAPSHOT=""
PLUGIN_BLAKE3=""
ASSET_SOURCE="release"
B3SUM=""
INSTALL_LOCK=""
LOCK_HELD=0
PLUGIN_STAGE=""
RECEIPT_STAGE=""

say() { printf '%s\n' "$*"; }
die() { say "aos-oracles: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

cleanup() {
  if [ "$STARTED_DAEMON" -eq 1 ] && have aos; then
    aos --principal default stop >/dev/null 2>&1 || true
  fi
  if [ "$LOCK_HELD" -eq 1 ] && [ -n "$INSTALL_LOCK" ]; then
    rm -rf "$INSTALL_LOCK"
    LOCK_HELD=0
  fi
  [ -z "$PLUGIN_STAGE" ] || rm -rf "$PLUGIN_STAGE"
  [ -z "$RECEIPT_STAGE" ] || rm -rf "$RECEIPT_STAGE"
  [ -z "$WORK" ] || rm -rf "$WORK"
}

on_signal() {
  code=$1
  trap - EXIT HUP INT TERM
  cleanup
  exit "$code"
}

trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

usage() {
  cat <<'EOF'
Usage: install.sh [options]

  --host HOST       install claude, codex, or grok (repeatable)
  --all             install every supported host
  --yes, -y         non-interactive; fresh CE profiles need OPENAI_API_KEY
  --oracle-version V exact signed oracle pack version (default: 0.2.0)
  --aos-channel C   install/follow the AOS stable, dev, or nightly channel
  --aos-version V   install an exact AOS calendar-semver release
  --local-assets D  use locally built capsules and pack manifests for testing
  --offline         initialize CE without update-network access
  --no-migrate-prompt
                    do not offer the optional Astrid state-import prompt
  --claude-auth M   api_key or subscription (non-interactive Claude setup)
  --claude-mode M   headless or repl (non-interactive Claude setup)
  --no-install-aos  fail instead of invoking the canonical AOS installer
  -h, --help
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --host)
      shift
      case "${1:-}" in
        claude|codex|grok) REQUESTED_HOSTS="$REQUESTED_HOSTS ${1}" ;;
        *) die "unknown host '${1:-}'" ;;
      esac
      ;;
    --all) ALL_HOSTS=1 ;;
    --yes|-y) ASSUME_YES=1 ;;
    --oracle-version)
      shift
      ORACLES_VERSION="${1:-}"
      ;;
    --aos-channel)
      shift
      AOS_CHANNEL="${1:-}"
      ;;
    --aos-version)
      shift
      AOS_VERSION="${1:-}"
      ;;
    --local-assets)
      shift
      LOCAL_ASSETS="${1:-}"
      ;;
    --offline) OFFLINE=1 ;;
    --no-migrate-prompt) NO_MIGRATE_PROMPT=1 ;;
    --claude-auth)
      shift
      CLAUDE_AUTH_MODE="${1:-}"
      ;;
    --claude-mode)
      shift
      CLAUDE_INTERACTION_MODE="${1:-}"
      ;;
    --no-install-aos) NO_INSTALL_AOS=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument '$1'" ;;
  esac
  shift
done

printf '%s\n' "$ORACLES_VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "invalid oracle version '$ORACLES_VERSION'"
[ -z "$AOS_CHANNEL" ] || [ -z "$AOS_VERSION" ] \
  || die "--aos-channel and --aos-version are mutually exclusive"
case "$AOS_CHANNEL" in
  ""|stable|dev|nightly) ;;
  *) die "invalid AOS channel '$AOS_CHANNEL'" ;;
esac
if [ -n "$AOS_VERSION" ]; then
  printf '%s\n' "$AOS_VERSION" \
    | grep -Eq '^(202[6-9]|20[3-9][0-9])\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
    || die "invalid AOS version '$AOS_VERSION'"
fi
case "$CLAUDE_AUTH_MODE" in
  api_key|subscription) ;;
  *) die "invalid Claude auth mode '$CLAUDE_AUTH_MODE'" ;;
esac
case "$CLAUDE_INTERACTION_MODE" in
  headless|repl) ;;
  *) die "invalid Claude interaction mode '$CLAUDE_INTERACTION_MODE'" ;;
esac
if [ "$CLAUDE_INTERACTION_MODE" = headless ] && [ "$CLAUDE_AUTH_MODE" = subscription ]; then
  die "Claude subscription authentication requires --claude-mode repl; headless mode requires --claude-auth api_key"
fi

if [ -n "$LOCAL_ASSETS" ]; then
  [ -d "$LOCAL_ASSETS" ] || die "local asset directory not found: $LOCAL_ASSETS"
  LOCAL_ASSETS=$(cd -- "$LOCAL_ASSETS" && pwd -P)
fi

platform() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os/$arch" in
    Darwin/arm64|Darwin/aarch64) printf 'darwin-arm64\n' ;;
    Darwin/x86_64) printf 'darwin-amd64\n' ;;
    Linux/aarch64|Linux/arm64) printf 'linux-arm64\n' ;;
    Linux/x86_64|Linux/amd64) printf 'linux-amd64\n' ;;
    *) return 1 ;;
  esac
}

sha256_file() {
  if have sha256sum; then sha256sum "$1" | awk '{print $1}'
  elif have shasum; then shasum -a 256 "$1" | awk '{print $1}'
  else return 1
  fi
}

ensure_b3sum() {
  if have b3sum; then
    B3SUM=$(command -v b3sum)
    return
  fi
  die "b3sum is required to verify signed oracle release checksums"
}

blake3_file() {
  "$B3SUM" "$1" | awk '{print $1}'
}

acquire_install_lock() {
  lock_root="$AOS_HOME_DIR/extensions/oracles"
  INSTALL_LOCK="$lock_root/.install.lock"
  mkdir -p "$lock_root"
  if ! mkdir "$INSTALL_LOCK" 2>/dev/null; then
    owner=""
    [ ! -r "$INSTALL_LOCK/pid" ] || owner=$(cat "$INSTALL_LOCK/pid" 2>/dev/null || true)
    if [ -n "$owner" ]; then
      die "another oracle installation is active for $AOS_HOME_DIR (pid $owner)"
    fi
    die "another oracle installation is active for $AOS_HOME_DIR"
  fi
  LOCK_HELD=1
  printf '%s\n' "$$" > "$INSTALL_LOCK/pid"
}

atomic_symlink() {
  target=$1
  destination=$2
  allow_regular=${3:-0}
  parent=${destination%/*}
  name=${destination##*/}
  temporary="$parent/.$name.$$"
  if [ -d "$destination" ] && [ ! -L "$destination" ]; then
    die "$destination is a directory"
  fi
  if [ -e "$destination" ] && [ ! -L "$destination" ] && [ "$allow_regular" -ne 1 ]; then
    die "$destination is not a symlink"
  fi
  rm -f "$temporary"
  ln -s "$target" "$temporary"
  case "$(uname -s)" in
    Darwin) mv -f -h "$temporary" "$destination" ;;
    Linux) mv -fT "$temporary" "$destination" ;;
    *) rm -f "$temporary"; die "unsupported platform for atomic symlink replacement" ;;
  esac
}

calendar_version_at_least() {
  actual=$1
  floor=$2
  awk -v actual="$actual" -v floor="$floor" 'BEGIN {
    split(actual, a, ".")
    split(floor, f, ".")
    ok = (a[1] > f[1]) ||
         (a[1] == f[1] && a[2] > f[2]) ||
         (a[1] == f[1] && a[2] == f[2] && a[3] >= f[3])
    exit !ok
  }'
}

ensure_aos() {
  if have aos && [ -z "$AOS_CHANNEL" ] && [ -z "$AOS_VERSION" ]; then return 0; fi
  [ "$NO_INSTALL_AOS" -eq 0 ] || die "Unicity AOS is required; run $AOS_INSTALL_URL"
  have curl || die "curl is required to install Unicity AOS"
  WORK=${WORK:-$(mktemp -d 2>/dev/null || mktemp -d -t aos-oracles)}
  installer="$WORK/aos-install.sh"
  curl -fsSL --max-time 60 "$AOS_INSTALL_URL" -o "$installer" \
    || die "could not download the canonical AOS installer"
  chmod 700 "$installer"
  set -- "$installer"
  [ "$ASSUME_YES" -eq 0 ] || set -- "$@" --yes
  if [ "$NO_MIGRATE_PROMPT" -eq 1 ] || [ "$ASSUME_YES" -eq 1 ]; then
    set -- "$@" --no-migrate-prompt
  fi
  [ -z "$AOS_CHANNEL" ] || set -- "$@" --channel "$AOS_CHANNEL"
  [ -z "$AOS_VERSION" ] || set -- "$@" --version "$AOS_VERSION"
  sh "$@"
  if [ -x "$AOS_HOME_DIR/bin/aos" ]; then
    PATH="$AOS_HOME_DIR/bin:$PATH"
    export PATH
  fi
  have aos || die "AOS installed but 'aos' is not on PATH; start a new shell and retry"
  if [ -n "$AOS_VERSION" ]; then
    installed=$(aos --version | awk 'NF { value = $NF } END { print value }')
    [ "$installed" = "$AOS_VERSION" ] \
      || die "requested Unicity AOS $AOS_VERSION but installer selected $installed"
  fi
}

detect_hosts() {
  if [ "$ALL_HOSTS" -eq 1 ]; then printf 'claude codex grok\n'; return; fi
  if [ -n "$REQUESTED_HOSTS" ]; then printf '%s\n' "$REQUESTED_HOSTS"; return; fi
  found=""
  have claude && found="$found claude"
  have codex && found="$found codex"
  have grok && found="$found grok"
  printf '%s\n' "$found"
}

run_init() {
  target="${1:-}"
  set -- aos --principal default init
  [ -z "$target" ] || set -- "$@" --target-principal "$target"
  [ "$OFFLINE" -eq 0 ] || set -- "$@" --offline
  if [ "$ASSUME_YES" -eq 1 ]; then
    set -- "$@" --yes
    if [ -n "${OPENAI_API_KEY:-}" ]; then
      ASTRID_VAR_OPENAI_API_KEY="$OPENAI_API_KEY" "$@"
    else
      "$@"
    fi
  elif [ -r /dev/tty ]; then
    "$@" </dev/tty
  else
    "$@"
  fi
}

daemon_is_live() {
  aos status --json >/dev/null 2>&1
}

ensure_base() {
  if ! daemon_is_live; then STARTED_DAEMON=1; fi
  run_init ""
}

principal_for() {
  case "$1" in
    claude) printf 'claude-code\n' ;;
    codex) printf 'codex-code\n' ;;
    grok) printf 'grok-code\n' ;;
  esac
}

capsules_for() {
  case "$1" in
    claude) printf '%s\n' astrid-mcp claude-install claude-runner ;;
    codex) printf '%s\n' astrid-mcp codex-install codex-runner ;;
    grok) printf '%s\n' astrid-mcp ;;
  esac
}

ensure_principal() {
  host=$1
  principal=$2
  if ! aos --principal default group show "$host" >/dev/null 2>&1; then
    aos --principal default group create "$host" \
      --caps 'self:*,delegate:self:*' \
      --description "Unicity AOS $host host family" >/dev/null
  fi
  if ! aos --principal default agent show "$principal" >/dev/null 2>&1; then
    aos --principal default agent create "$principal" --group "$host" \
      --inherit-from default --yes >/dev/null
  fi
  run_init "$principal"
}

ensure_cosign() {
  if have cosign; then COSIGN=$(command -v cosign); return; fi
  have curl || die "curl is required to fetch the Sigstore verifier"
  WORK=${WORK:-$(mktemp -d 2>/dev/null || mktemp -d -t aos-oracles)}
  target=$(platform) || die "unsupported platform for Sigstore verification"
  case "$target" in
    darwin-arm64) digest=94b42a9e697be95675f6160ab031a9a5f1ec1e646d6f648d7b2f5cd59ececbc5 ;;
    darwin-amd64) digest=14d2678dfbfde18798151e86fbd91ebdadbb7424b18412a42a155dd8a2df4c7a ;;
    linux-arm64) digest=2ec865872e331c32fd12b08dae15332d3f92c0aa029219589684a4903ca85d11 ;;
    linux-amd64) digest=ae1ecd212663f3693ad9edf8b1a183900c9a52d3155ba6e354237f9a0f6463fc ;;
  esac
  COSIGN="$WORK/cosign"
  curl -fsSL --max-time 120 \
    "https://github.com/sigstore/cosign/releases/download/$COSIGN_VERSION/cosign-$target" \
    -o "$COSIGN" || die "could not download the Sigstore verifier"
  [ "$(sha256_file "$COSIGN")" = "$digest" ] || die "Sigstore verifier checksum mismatch"
  chmod 700 "$COSIGN"
}

verify_release_asset() {
  asset=$1
  bundle=$2
  identity="https://github.com/$ORACLES_REPO/.github/workflows/release.yml@refs/tags/v$ORACLES_VERSION"
  "$COSIGN" verify-blob --bundle "$bundle" \
    --certificate-identity "$identity" \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    --use-signed-timestamps "$asset" >/dev/null \
    || die "Sigstore verification failed for $(basename "$asset")"
}

download_verified() {
  name=$1
  out=$2
  base="https://github.com/$ORACLES_REPO/releases/download/v$ORACLES_VERSION"
  curl -fsSL --max-time 120 "$base/$name" -o "$out" \
    || die "could not download $name from v$ORACLES_VERSION"
  curl -fsSL --max-time 60 "$base/$name.sigstore.json" -o "$out.sigstore.json" \
    || die "could not download the Sigstore bundle for $name"
  verify_release_asset "$out" "$out.sigstore.json"
}

validate_checksum_manifest() {
  manifest=$1
  [ -s "$manifest" ] || die "release checksum manifest is empty"
  if grep -Ev '^[0-9a-f]{64}  [A-Za-z0-9][A-Za-z0-9._-]*$' "$manifest" >/dev/null; then
    die "release checksum manifest has an invalid entry"
  fi
  names="$WORK/checksum-names.txt"
  awk '{print $2}' "$manifest" | LC_ALL=C sort > "$names"
  if [ -n "$(uniq -d "$names")" ]; then
    die "release checksum manifest contains duplicate asset names"
  fi
  while IFS= read -r name; do
    case "$name" in
      astrid-mcp.capsule|\
      claude-install.capsule|claude-pack.toml|claude-runner.capsule|\
      codex-install.capsule|codex-pack.toml|codex-runner.capsule|\
      grok-pack.toml|aos-oracle-plugins.tar.gz|runtime-compatibility.toml) ;;
      *) die "release checksum manifest names an unknown asset: $name" ;;
    esac
  done < "$names"
}

expected_blake3() {
  name=$1
  awk -v name="$name" '$2 == name { print $1; found = 1 } END { exit !found }' \
    "$RELEASE_STAGE/BLAKE3SUMS.txt"
}

verify_blake3() {
  path=$1
  name=$2
  expected=$(expected_blake3 "$name") \
    || die "release checksum manifest has no digest for $name"
  actual=$(blake3_file "$path")
  printf '%s\n' "$actual" | grep -Eq '^[0-9a-f]{64}$' \
    || die "b3sum returned an invalid digest for $name"
  [ "$actual" = "$expected" ] || die "BLAKE3 checksum mismatch for $name"
}

validate_plugin_archive() {
  archive=$1
  members="$WORK/plugin-members.txt"
  entries="$WORK/plugin-entries.txt"
  tar -tzf "$archive" > "$members" || die "could not inspect the plugin snapshot"
  tar -tvzf "$archive" > "$entries" || die "could not inspect plugin snapshot entry types"
  if grep -Ev '^[-d]' "$entries" >/dev/null; then
    die "plugin snapshot contains a link or special entry"
  fi
  while IFS= read -r member; do
    case "$member" in
      ""|*[!A-Za-z0-9_./@+-]*) die "plugin snapshot contains an unsafe path: $member" ;;
      /*|../*|*/../*|*/..) die "plugin snapshot contains an unsafe path: $member" ;;
    esac
  done < "$members"
}

stage_release_metadata() {
  WORK=${WORK:-$(mktemp -d 2>/dev/null || mktemp -d -t aos-oracles)}
  RELEASE_STAGE="$WORK/release"
  mkdir -p "$RELEASE_STAGE"
  if [ -n "$LOCAL_ASSETS" ]; then
    ASSET_SOURCE=local
    for asset in aos-oracle-plugins.tar.gz BLAKE3SUMS.txt runtime-compatibility.toml; do
      cp "$LOCAL_ASSETS/$asset" "$RELEASE_STAGE/$asset" \
        || die "local release asset is missing: $asset"
    done
  else
    ensure_cosign
    for asset in aos-oracle-plugins.tar.gz BLAKE3SUMS.txt runtime-compatibility.toml; do
      download_verified "$asset" "$RELEASE_STAGE/$asset"
    done
  fi
  validate_checksum_manifest "$RELEASE_STAGE/BLAKE3SUMS.txt"
  verify_blake3 "$RELEASE_STAGE/aos-oracle-plugins.tar.gz" aos-oracle-plugins.tar.gz
  verify_blake3 "$RELEASE_STAGE/runtime-compatibility.toml" runtime-compatibility.toml
  PLUGIN_BLAKE3=$(expected_blake3 aos-oracle-plugins.tar.gz)
  validate_plugin_archive "$RELEASE_STAGE/aos-oracle-plugins.tar.gz"
}

prepare_plugin_snapshot() {
  [ -z "$PLUGIN_SNAPSHOT" ] || return 0
  archive="$RELEASE_STAGE/aos-oracle-plugins.tar.gz"
  plugins_root="$AOS_HOME_DIR/extensions/oracles/plugins"
  destination="$plugins_root/$ORACLES_VERSION"
  stage="$plugins_root/.${ORACLES_VERSION}.tmp.$$"
  PLUGIN_STAGE=$stage
  mkdir -p "$plugins_root"
  rm -rf "$stage"
  mkdir -p "$stage"
  tar -xzf "$archive" -C "$stage" || die "could not extract the plugin snapshot"
  if find "$stage" ! -type f ! -type d -print -quit | grep . >/dev/null; then
    die "plugin snapshot extracted a link or special entry"
  fi
  for required in \
    .agents/plugins/marketplace.json \
    .claude-plugin/marketplace.json \
    .grok-plugin/marketplace.json \
    plugins/claude/.claude-plugin/plugin.json \
    plugins/grok/.grok-plugin/plugin.json \
    plugins/unicity-aos/.codex-plugin/plugin.json
  do
    [ -f "$stage/$required" ] && [ ! -L "$stage/$required" ] \
      || die "plugin snapshot is missing a regular $required"
  done
  if [ -e "$destination" ]; then
    if find "$destination" ! -type f ! -type d -print -quit | grep . >/dev/null; then
      die "installed plugin snapshot $ORACLES_VERSION contains a link or special entry"
    fi
    diff -qr "$stage" "$destination" >/dev/null \
      || die "installed plugin snapshot $ORACLES_VERSION differs from the staged release"
    rm -rf "$stage"
  else
    mv "$stage" "$destination" || die "could not activate the plugin snapshot"
  fi
  PLUGIN_STAGE=""
  PLUGIN_SNAPSHOT="$destination"
}

validate_pack() {
  host=$1
  principal=$2
  pack=$3
  grep -Fqx "host = \"$host\"" "$pack" || die "pack host mismatch"
  grep -Fqx "principal = \"$principal\"" "$pack" || die "pack principal mismatch"
  grep -Fqx "version = \"$ORACLES_VERSION\"" "$pack" || die "pack version mismatch"
  aos_floor=$(sed -n 's/^aos-version = ">=\([^"]*\)"$/\1/p' "$pack")
  [ -n "$aos_floor" ] || die "signed pack has no valid AOS version floor"
  printf '%s\n' "$aos_floor" \
    | grep -Eq '^20[0-9][0-9]\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
    || die "signed pack has invalid AOS version floor '$aos_floor'"
  installed_aos=$(aos --version | awk 'NF { value = $NF } END { print value }')
  printf '%s\n' "$installed_aos" \
    | grep -Eq '^20[0-9][0-9]\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
    || die "could not determine the installed Unicity AOS version"
  calendar_version_at_least "$installed_aos" "$aos_floor" \
    || die "Unicity AOS $installed_aos does not satisfy pack requirement >=$aos_floor"
  actual=$(sed -n '/^\[\[capsule\]\]$/,/^$/s/^name = "\([^"]*\)"/\1/p' "$pack")
  expected=$(capsules_for "$host")
  [ "$actual" = "$expected" ] || die "signed $host pack capsule set is not the expected release set"
}

stage_pack() {
  host=$1
  WORK=${WORK:-$(mktemp -d 2>/dev/null || mktemp -d -t aos-oracles)}
  stage="$WORK/$host"
  mkdir -p "$stage"
  if [ -n "$LOCAL_ASSETS" ]; then
    cp "$LOCAL_ASSETS/$host.toml" "$stage/Pack.toml" \
      || die "local $host pack manifest is missing"
    for capsule in $(capsules_for "$host"); do
      cp "$LOCAL_ASSETS/$capsule.capsule" "$stage/$capsule.capsule" \
        || die "local capsule is missing: $capsule"
    done
  else
    ensure_cosign
    download_verified "$host-pack.toml" "$stage/Pack.toml"
    for capsule in $(capsules_for "$host"); do
      download_verified "$capsule.capsule" "$stage/$capsule.capsule"
    done
  fi
  verify_blake3 "$stage/Pack.toml" "$host-pack.toml"
  for capsule in $(capsules_for "$host"); do
    verify_blake3 "$stage/$capsule.capsule" "$capsule.capsule"
  done
  STAGED_PACK=$stage
}

install_pack() {
  host=$1
  principal=$(principal_for "$host")
  ensure_principal "$host" "$principal"
  stage_pack "$host"
  stage=$STAGED_PACK
  validate_pack "$host" "$principal" "$stage/Pack.toml"

  for capsule in $(capsules_for "$host"); do
    if [ "$ASSUME_YES" -eq 1 ]; then
      if [ "$host" = claude ] && [ "$capsule" = claude-runner ]; then
        if [ "$CLAUDE_AUTH_MODE" = api_key ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
          die "ANTHROPIC_API_KEY is required for --yes --host claude with --claude-auth api_key"
        fi
        ASTRID_VAR_INTERACTION_MODE="$CLAUDE_INTERACTION_MODE" \
          ASTRID_VAR_AUTH_MODE="$CLAUDE_AUTH_MODE" \
          ASTRID_VAR_API_KEY="${ANTHROPIC_API_KEY:-}" \
          aos --principal "$principal" capsule install \
            "$stage/$capsule.capsule" --yes
      else
        aos --principal "$principal" capsule install "$stage/$capsule.capsule" --yes
      fi
    elif [ -r /dev/tty ]; then
      aos --principal "$principal" capsule install "$stage/$capsule.capsule" </dev/tty
    else
      aos --principal "$principal" capsule install "$stage/$capsule.capsule"
    fi
  done

  set -- aos --principal default agent modify "$principal"
  for capsule in $(capsules_for "$host"); do
    set -- "$@" --add-capsule "$capsule"
  done
  "$@" >/dev/null
  say "✓ $host oracle capsules ready as $principal"
}

install_plugin() {
  host=$1
  case "$host" in
    claude)
      have claude || die "Claude Code is not installed"
      claude plugin marketplace remove unicity-aos-oracles >/dev/null 2>&1 || true
      claude plugin marketplace add "$PLUGIN_SNAPSHOT" >/dev/null
      claude plugin install unicity-aos@unicity-aos-oracles >/dev/null
      claude plugin uninstall astrid@astrid-oracles >/dev/null 2>&1 || true
      ;;
    codex)
      have codex || die "Codex is not installed"
      if codex plugin marketplace list 2>/dev/null \
        | awk '$1 == "unicity-aos-oracles" { found = 1 } END { exit !found }'
      then
        codex plugin marketplace remove unicity-aos-oracles >/dev/null 2>&1 || true
        codex plugin marketplace add "$PLUGIN_SNAPSHOT" >/dev/null
      else
        codex plugin marketplace add "$PLUGIN_SNAPSHOT" >/dev/null
      fi
      codex plugin add unicity-aos@unicity-aos-oracles >/dev/null
      codex plugin remove astrid@astrid-oracles >/dev/null 2>&1 || true
      ;;
    grok)
      have grok || die "Grok Build is not installed"
      grok plugin install "$PLUGIN_SNAPSHOT/plugins/grok" --trust >/dev/null
      grok plugin uninstall astrid >/dev/null 2>&1 || true
      ;;
  esac
  say "✓ $host marketplace plugin installed"
}

write_receipt() {
  host=$1
  principal=$2
  pack_stage=$3
  receipt_root="$AOS_HOME_DIR/extensions/oracles/$host"
  releases="$receipt_root/releases"
  destination="$releases/$ORACLES_VERSION"
  stage="$receipt_root/.receipt-${ORACLES_VERSION}.$$"
  RECEIPT_STAGE=$stage
  mkdir -p "$releases"
  rm -rf "$stage"
  mkdir -p "$stage"
  cp "$pack_stage/Pack.toml" "$stage/Pack.lock"
  cp "$RELEASE_STAGE/BLAKE3SUMS.txt" "$stage/BLAKE3SUMS.txt"
  cp "$RELEASE_STAGE/runtime-compatibility.toml" "$stage/runtime-compatibility.toml"
  for bundle in \
    "$pack_stage/Pack.toml.sigstore.json" \
    "$pack_stage"/*.capsule.sigstore.json \
    "$RELEASE_STAGE/BLAKE3SUMS.txt.sigstore.json" \
    "$RELEASE_STAGE/runtime-compatibility.toml.sigstore.json" \
    "$RELEASE_STAGE/aos-oracle-plugins.tar.gz.sigstore.json"
  do
    [ -f "$bundle" ] || continue
    cp "$bundle" "$stage/$(basename "$bundle")"
  done
  {
    printf 'schema-version = 1\n'
    printf 'oracle-version = "%s"\n' "$ORACLES_VERSION"
    printf 'host = "%s"\n' "$host"
    printf 'principal = "%s"\n' "$principal"
    printf 'source = "%s"\n' "$ASSET_SOURCE"
    printf 'plugin-snapshot = "../../../plugins/%s"\n' "$ORACLES_VERSION"
    printf 'plugin-blake3 = "%s"\n' "$PLUGIN_BLAKE3"
  } > "$stage/Receipt.toml"
  chmod 700 "$stage"
  find "$stage" -type f -exec chmod 600 {} \;
  if [ -e "$destination" ]; then
    if find "$destination" ! -type f ! -type d -print -quit | grep . >/dev/null; then
      die "installed $host receipt $ORACLES_VERSION contains a link or special entry"
    fi
    diff -qr "$stage" "$destination" >/dev/null \
      || die "installed $host receipt $ORACLES_VERSION differs from the staged release"
    rm -rf "$stage"
  else
    mv "$stage" "$destination" || die "could not commit the $host oracle receipt"
  fi
  RECEIPT_STAGE=""

  atomic_symlink "releases/$ORACLES_VERSION" "$receipt_root/current"
  atomic_symlink current/Pack.lock "$receipt_root/Pack.lock" 1
  if [ -f "$destination/Pack.toml.sigstore.json" ]; then
    atomic_symlink current/Pack.toml.sigstore.json \
      "$receipt_root/Pack.lock.sigstore.json" 1
  else
    rm -f "$receipt_root/Pack.lock.sigstore.json"
  fi
  say "✓ $host oracle pack $ORACLES_VERSION committed"
}

ensure_b3sum
hosts=$(detect_hosts)
[ -n "$(printf '%s' "$hosts" | tr -d ' ')" ] \
  || die "no supported host detected; pass --host claude, --host codex, or --host grok"
ensure_aos
acquire_install_lock
stage_release_metadata
ensure_base
for host in $hosts; do
  install_pack "$host"
  prepare_plugin_snapshot
  install_plugin "$host"
  write_receipt "$host" "$(principal_for "$host")" "$STAGED_PACK"
done

say "Unicity AOS oracle installation complete. Start a new host session to load the plugin."
