#!/usr/bin/env sh
# Install signed Unicity AOS oracle packs and their host marketplace plugins.
set -eu
umask 077

ORACLES_REPO="${AOS_ORACLES_REPO:-unicity-aos/oracles}"
ORACLES_VERSION="${AOS_ORACLES_VERSION:-0.2.1}"
AOS_INSTALL_URL="${AOS_INSTALL_URL:-https://aos.unicity.ai/base-install.sh}"
AOS_HOME_DIR="${AOS_HOME:-$HOME/.aos}"
AOS_CHANNEL=""
AOS_VERSION=""
COSIGN_VERSION=v3.1.1
ASSUME_YES=0
ALL_HOSTS=0
NO_INSTALL_AOS=0
SKIP_HOST_PLUGIN=0
PLUGINS_ONLY=0
REQUESTED_HOSTS=""
LOCAL_ASSETS="${AOS_ORACLE_ASSETS:-}"
WORK=""
COSIGN=""
RELEASE_STAGE=""
PLUGIN_SNAPSHOT=""
PLUGIN_BLAKE3=""
ASSET_SOURCE="release"
B3SUM=""
INSTALL_LOCK=""
LOCK_HELD=0
LOCK_BACKEND=""
PLUGIN_STAGE=""
RECEIPT_STAGE=""
PREVIOUS_BINDINGS=""
CURRENT_PACK_BINDINGS=""

say() { printf '%s\n' "$*"; }
die() { say "aos-oracles: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

release_install_lock() {
  [ "$LOCK_HELD" -eq 1 ] && [ -n "$INSTALL_LOCK" ] || return 0
  owner=""
  if [ -f "$INSTALL_LOCK" ] && [ ! -L "$INSTALL_LOCK" ] \
    && IFS= read -r owner < "$INSTALL_LOCK" \
    && [ "$owner" = "$$" ]
  then
    rm -f "$INSTALL_LOCK"
  fi
  if [ -n "$LOCK_BACKEND" ]; then
    exec 9>&-
  fi
  LOCK_HELD=0
  LOCK_BACKEND=""
}

cleanup() {
  release_install_lock
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
  --yes, -y         non-interactive host-pack provisioning
  --oracle-version V exact signed oracle pack version (default: 0.2.1)
  --aos-channel C   install/follow the AOS stable, dev, or nightly channel
  --aos-version V   install an exact AOS calendar-semver release
  --local-assets D  use locally built capsules and pack manifests for testing
  --aos-installer S use an alternate AOS installer URL or local path for testing
  --plugins-only    install selected host marketplace plugins; provision on host start
  --no-install-aos  fail instead of invoking the canonical AOS installer
  --skip-host-plugin
                     provision capsules/receipt without reinstalling the active host plugin
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
    --aos-installer)
      shift
      AOS_INSTALL_URL="${1:-}"
      [ -n "$AOS_INSTALL_URL" ] || die "--aos-installer requires a URL or local path"
      ;;
    --plugins-only) PLUGINS_ONLY=1 ;;
    --no-install-aos) NO_INSTALL_AOS=1 ;;
    --skip-host-plugin) SKIP_HOST_PLUGIN=1 ;;
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
if [ -n "$LOCAL_ASSETS" ]; then
  [ -d "$LOCAL_ASSETS" ] || die "local asset directory not found: $LOCAL_ASSETS"
  LOCAL_ASSETS=$(cd -- "$LOCAL_ASSETS" && pwd -P)
fi

require_commands() {
  missing=""
  for command in \
    awk basename cat chmod cp diff find grep ln mkdir mktemp mv pwd rm sed sort tar tr uniq uname
  do
    have "$command" || missing="$missing $command"
  done
  [ -z "$missing" ] || die "missing required commands:$missing"
  have flock || have lockf \
    || die "missing required command: flock or lockf"
}

require_commands

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
  if [ -n "$LOCAL_ASSETS" ]; then
    die "b3sum is required to verify unsigned local oracle assets"
  fi
  # Every downloaded release asset is independently verified by Sigstore
  # against the pinned release-workflow identity. BLAKE3 is an additional
  # byte-for-byte check when b3sum is available, not an installation
  # prerequisite for an otherwise authenticated release.
  B3SUM=""
}

blake3_file() {
  "$B3SUM" "$1" | awk '{print $1}'
}

acquire_install_lock() {
  lock_root="$AOS_HOME_DIR/extensions/oracles"
  INSTALL_LOCK="$lock_root/.install.lock"
  for lock_parent in "$AOS_HOME_DIR" "$AOS_HOME_DIR/extensions" "$lock_root"; do
    [ ! -L "$lock_parent" ] || die "refusing symlinked install lock path: $lock_parent"
  done
  mkdir -p "$lock_root"
  chmod 700 "$AOS_HOME_DIR" "$AOS_HOME_DIR/extensions" "$lock_root"
  [ ! -L "$INSTALL_LOCK" ] || die "refusing symlinked oracle install lock"
  [ ! -e "$INSTALL_LOCK" ] || [ -f "$INSTALL_LOCK" ] \
    || die "oracle install lock is not a regular file"

  exec 9>>"$INSTALL_LOCK"
  if have flock; then
    lock_command=flock
    lock_acquired=0
    flock -n 9 || lock_acquired=$?
  else
    lock_command=lockf
    lock_acquired=0
    lockf -s -t 0 9 || lock_acquired=$?
  fi
  if [ "$lock_acquired" -ne 0 ]; then
    owner=""
    IFS= read -r owner < "$INSTALL_LOCK" || owner=""
    exec 9>&-
    case "$owner" in
      ''|*[!0-9]*) die "another oracle installation is active for $AOS_HOME_DIR" ;;
      *) die "another oracle installation is active for $AOS_HOME_DIR (pid $owner)" ;;
    esac
  fi
  : > "$INSTALL_LOCK"
  printf '%s\n' "$$" > "$INSTALL_LOCK"
  LOCK_BACKEND=$lock_command

  LOCK_HELD=1
  if ! chmod 600 "$INSTALL_LOCK"; then
    release_install_lock
    die "could not secure oracle install lock owner"
  fi
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
  if [ -x "$AOS_HOME_DIR/bin/aos" ] && [ -z "$AOS_CHANNEL" ] && [ -z "$AOS_VERSION" ]; then
    PATH="$AOS_HOME_DIR/bin:$PATH"
    export PATH
    return 0
  fi
  if [ "$NO_INSTALL_AOS" -eq 1 ] && have aos \
    && [ -z "$AOS_CHANNEL" ] && [ -z "$AOS_VERSION" ]
  then
    return 0
  fi
  [ "$NO_INSTALL_AOS" -eq 0 ] || die "Unicity AOS is required; run $AOS_INSTALL_URL"
  have curl || die "curl is required to install Unicity AOS"
  WORK=${WORK:-$(mktemp -d 2>/dev/null || mktemp -d -t aos-oracles)}
  installer="$WORK/aos-install.sh"
  if [ -f "$AOS_INSTALL_URL" ] && [ ! -L "$AOS_INSTALL_URL" ]; then
    cp "$AOS_INSTALL_URL" "$installer"
  else
    case "$AOS_INSTALL_URL" in
      /*)
        die "local AOS installer is not a regular file: $AOS_INSTALL_URL"
        ;;
      file://*)
        local_installer=${AOS_INSTALL_URL#file://}
        [ -f "$local_installer" ] && [ ! -L "$local_installer" ] \
          || die "local AOS installer is not a regular file: $local_installer"
        cp "$local_installer" "$installer"
        ;;
      *)
        curl -fsSL --max-time 60 "$AOS_INSTALL_URL" -o "$installer" \
          || die "could not download the canonical AOS installer"
        ;;
    esac
  fi
  chmod 700 "$installer"
  set -- "$installer"
  [ "$ASSUME_YES" -eq 0 ] || set -- "$@" --yes
  [ -z "$AOS_CHANNEL" ] || set -- "$@" --channel "$AOS_CHANNEL"
  [ -z "$AOS_VERSION" ] || set -- "$@" --version "$AOS_VERSION"
  sh "$@"
  if [ -x "$AOS_HOME_DIR/bin/aos" ]; then
    PATH="$AOS_HOME_DIR/bin:$PATH"
    export PATH
  fi
  [ -x "$AOS_HOME_DIR/bin/aos" ] \
    || die "AOS installer did not provision $AOS_HOME_DIR/bin/aos"
  PATH="$AOS_HOME_DIR/bin:$PATH"
  export PATH
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

select_hosts() {
  found=$(detect_hosts)
  [ -n "$(printf '%s' "$found" | tr -d ' ')" ] \
    || die "no supported host detected; pass --host claude, --host codex, or --host grok"
  if [ "$ALL_HOSTS" -eq 1 ] || [ -n "$REQUESTED_HOSTS" ] || [ "$ASSUME_YES" -eq 1 ]; then
    printf '%s\n' "$found"
    return 0
  fi
  [ -r /dev/tty ] \
    || die "host selection requires an interactive terminal; pass --yes, --all, or --host HOST"
  selected=""
  for host in $found; do
    printf 'Install the Unicity AOS plugin for %s? [Y/n] ' "$host" >/dev/tty
    answer=""
    IFS= read -r answer </dev/tty || true
    case "$answer" in
      ""|y|Y|yes|YES|Yes) selected="$selected $host" ;;
    esac
  done
  [ -n "$(printf '%s' "$selected" | tr -d ' ')" ] \
    || die "no host plugins selected"
  printf '%s\n' "$selected"
}

daemon_is_live() {
  status=$(aos status --json 2>/dev/null || true)
  printf '%s' "$status" \
    | grep -Eq '"state"[[:space:]]*:[[:space:]]*"running"'
}

ensure_base() {
  daemon_was_live=1
  if ! daemon_is_live; then daemon_was_live=0; fi
  if [ "$daemon_was_live" -eq 0 ]; then
    version=$(aos --version | awk 'NF { value = $NF } END { print value }')
    cli_artifact="$AOS_HOME_DIR/releases/$version/capsules/aos-cli.capsule"
    [ -f "$cli_artifact" ] && [ ! -L "$cli_artifact" ] \
      || die "installed Unicity AOS $version is missing aos-cli.capsule"
    aos --principal default capsule install "$cli_artifact" --yes </dev/null
    say "Starting Unicity CE..."
    aos --principal default start >/dev/null
    daemon_is_live \
      || die "Unicity CE did not become reachable after the runtime reported readiness"
  fi
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
    claude|codex|grok) printf '%s\n' aos-mcp ;;
  esac
}

pack_capsules_tsv() {
  pc_pack=$1
  awk '
    function emit() {
      if (!inside) return
      if (name == "" || asset != name ".capsule" || hash == "" || seen[name]++) exit 2
      print name " " hash
      name = ""
      asset = ""
      hash = ""
    }
    /^\[\[capsule\]\]$/ { emit(); inside = 1; next }
    inside && /^name = "[A-Za-z0-9][A-Za-z0-9._-]*"$/ {
      name = $0
      sub(/^name = "/, "", name)
      sub(/"$/, "", name)
      next
    }
    inside && /^asset = "[A-Za-z0-9][A-Za-z0-9._-]*\.capsule"$/ {
      asset = $0
      sub(/^asset = "/, "", asset)
      sub(/"$/, "", asset)
      next
    }
    inside && /^wasm-blake3 = "[0-9a-f]+"$/ {
      hash = $0
      sub(/^wasm-blake3 = "/, "", hash)
      sub(/"$/, "", hash)
      if (length(hash) != 64) exit 2
      next
    }
    END { emit() }
  ' "$pc_pack" || die "pack has invalid capsule ownership metadata"
}

write_managed_capsules() {
  wm_bindings=$1
  wm_output=$2
  {
    printf 'schema-version = 1\n'
    while read -r wm_name wm_hash wm_extra; do
      [ -n "$wm_name" ] || continue
      [ -z "${wm_extra:-}" ] || die "invalid managed capsule record"
      printf '\n[[capsule]]\n'
      printf 'name = "%s"\n' "$wm_name"
      printf 'wasm-hash = "%s"\n' "$wm_hash"
    done < "$wm_bindings"
  } > "$wm_output"
}

load_capsule_record() {
  cr_principal=$1
  cr_capsule=$2
  cr_record=$(aos capsule show "$cr_capsule" --agent "$cr_principal" \
    --format toml 2>/dev/null) || return 1
  CAPSULE_HASH=$(printf '%s\n' "$cr_record" \
    | sed -n 's/^wasm_hash = "\([0-9a-f]*\)"$/\1/p')
  printf '%s\n' "$CAPSULE_HASH" | grep -Eq '^[0-9a-f]{64}$' || return 1
  CAPSULE_SOURCE=$(printf '%s\n' "$cr_record" \
    | sed -n 's/^source = "\([^"]*\)"$/\1/p')
  CAPSULE_INSTALLED_AT=$(printf '%s\n' "$cr_record" \
    | sed -n 's/^installed_at = "\([^"]*\)"$/\1/p')
  CAPSULE_UPDATED_AT=$(printf '%s\n' "$cr_record" \
    | sed -n 's/^updated_at = "\([^"]*\)"$/\1/p')
}

binding_hash() {
  bh_file=$1
  bh_name=$2
  awk -v name="$bh_name" '$1 == name { print $2; found = 1 } END { exit !found }' \
    "$bh_file"
}

append_binding() {
  ab_file=$1
  ab_name=$2
  ab_hash=$3
  printf '%s\n' "$ab_name" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._-]*$' \
    || die "invalid capsule name in ownership state: $ab_name"
  printf '%s\n' "$ab_hash" | grep -Eq '^[0-9a-f]{64}$' \
    || die "invalid capsule hash in ownership state: $ab_name"
  ab_existing=$(binding_hash "$ab_file" "$ab_name" 2>/dev/null || true)
  if [ -n "$ab_existing" ]; then
    [ "$ab_existing" = "$ab_hash" ] \
      || die "conflicting ownership records for capsule $ab_name"
    return 0
  fi
  printf '%s %s\n' "$ab_name" "$ab_hash" >> "$ab_file"
}

legacy_v020_hash() {
  case "$1" in
    aos-mcp) printf 'a2e772db86cbbc1a19a86033254f9379a01fe2c07258bc419793316f9d40e95e\n' ;;
    claude-install) printf 'b5dd4e2beb234163419088187a87603a42284805de6e288b5450b712e24dfd2f\n' ;;
    claude-runner) printf '19adab7d37a9be54a0a1866349594461f8116c65612134c124aae94fa79c3c63\n' ;;
    codex-install) printf '6c510fd2185311dd6de4fd44adb19f9ff19f2251adcad16ff18d859a434e8593\n' ;;
    codex-runner) printf '0b9473ccba844bce95fff41126c620107f71d630ee0e1d0dd23e5a542613642c\n' ;;
    *) return 1 ;;
  esac
}

same_install_window() {
  sw_candidate=$1
  sw_anchor=$2
  awk -v candidate="$sw_candidate" -v anchor="$sw_anchor" 'BEGIN {
    iso = "^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:"
    if (candidate !~ iso || anchor !~ iso) exit 1
    if (substr(candidate, 1, 10) != substr(anchor, 1, 10)) exit 1
    ch = substr(candidate, 12, 2) + 0
    cm = substr(candidate, 15, 2) + 0
    ah = substr(anchor, 12, 2) + 0
    am = substr(anchor, 15, 2) + 0
    delta = (ah * 60 + am) - (ch * 60 + cm)
    exit !(delta >= 0 && delta <= 5)
  }'
}

append_legacy_ce_bindings() {
  lc_principal=$1
  lc_output=$2
  load_capsule_record "$lc_principal" aos-mcp || return 0
  lc_anchor=$CAPSULE_INSTALLED_AT
  [ -n "$lc_anchor" ] || return 0

  for lc_manifest in "$AOS_HOME_DIR"/releases/*/Distro.toml; do
    [ -f "$lc_manifest" ] && [ ! -L "$lc_manifest" ] || continue
    grep -Fqx 'id = "unicity-ce"' "$lc_manifest" || continue
    lc_release=${lc_manifest%/Distro.toml}
    lc_version=${lc_release##*/}
    printf '%s\n' "$lc_version" \
      | grep -Eq '^20[0-9][0-9]\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || continue
    grep -Fqx "version = \"$lc_version\"" "$lc_manifest" || continue
    lc_names=$(sed -n '/^\[\[capsule\]\]$/,/^$/s/^name = "\([A-Za-z0-9][A-Za-z0-9._-]*\)"$/\1/p' \
      "$lc_manifest")
    for lc_name in $lc_names; do
      [ "$lc_name" != aos-mcp ] || continue
      load_capsule_record "$lc_principal" "$lc_name" || continue
      [ "$CAPSULE_SOURCE" = "$lc_release/capsules/$lc_name.capsule" ] || continue
      if [ -z "$CAPSULE_INSTALLED_AT" ] \
        || [ "$CAPSULE_INSTALLED_AT" != "$CAPSULE_UPDATED_AT" ] \
        || ! same_install_window "$CAPSULE_INSTALLED_AT" "$lc_anchor"
      then
        continue
      fi
      append_binding "$lc_output" "$lc_name" "$CAPSULE_HASH"
    done
  done
}

load_previous_bindings() {
  lp_host=$1
  lp_principal=$2
  lp_root="$AOS_HOME_DIR/extensions/oracles/$lp_host"
  lp_pack="$lp_root/Pack.lock"
  lp_receipt="$lp_root/current/Receipt.toml"
  PREVIOUS_BINDINGS="$WORK/previous-$lp_host.bindings"
  : > "$PREVIOUS_BINDINGS"
  [ -r "$lp_pack" ] || return 0
  [ -r "$lp_receipt" ] || die "installed $lp_host Oracle pack has no receipt"
  grep -Fqx "host = \"$lp_host\"" "$lp_pack" \
    || die "installed $lp_host Oracle pack has the wrong host"
  grep -Fqx "principal = \"$lp_principal\"" "$lp_pack" \
    || die "installed $lp_host Oracle pack has the wrong principal"
  grep -Fqx "host = \"$lp_host\"" "$lp_receipt" \
    || die "installed $lp_host Oracle receipt has the wrong host"
  grep -Fqx "principal = \"$lp_principal\"" "$lp_receipt" \
    || die "installed $lp_host Oracle receipt has the wrong principal"

  lp_managed="$lp_root/current/ManagedCapsules.toml"
  if [ -r "$lp_managed" ]; then
    pack_capsules_tsv "$lp_pack" > "$PREVIOUS_BINDINGS"
    lp_expected="$WORK/expected-$lp_host-managed.toml"
    write_managed_capsules "$PREVIOUS_BINDINGS" "$lp_expected"
    diff -q "$lp_expected" "$lp_managed" >/dev/null \
      || die "installed $lp_host managed-capsule receipt is invalid"
    return 0
  fi

  if ! grep -Fqx 'oracle-version = "0.2.0"' "$lp_receipt" \
    || ! grep -Fqx 'version = "0.2.0"' "$lp_pack"
  then
    say "Preserving untracked capsules from the installed $lp_host Oracle pack."
    return 0
  fi
  lp_names=$(sed -n '/^\[\[capsule\]\]$/,/^$/s/^name = "\([A-Za-z0-9][A-Za-z0-9._-]*\)"$/\1/p' \
    "$lp_pack")
  for lp_name in $lp_names; do
    lp_hash=$(legacy_v020_hash "$lp_name") \
      || die "installed v0.2.0 $lp_host pack names an unknown capsule: $lp_name"
    append_binding "$PREVIOUS_BINDINGS" "$lp_name" "$lp_hash"
  done
  append_legacy_ce_bindings "$lp_principal" "$PREVIOUS_BINDINGS"
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
      --yes >/dev/null
  fi
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
  if grep -Ev '^[0-9a-f]{64}  (\./)?[A-Za-z0-9][A-Za-z0-9._-]*$' "$manifest" >/dev/null; then
    die "release checksum manifest has an invalid entry"
  fi
  names="$WORK/checksum-names.txt"
  awk '{ name = $2; sub(/^\.\//, "", name); print name }' "$manifest" \
    | LC_ALL=C sort > "$names"
  if [ -n "$(uniq -d "$names")" ]; then
    die "release checksum manifest contains duplicate asset names"
  fi
  while IFS= read -r name; do
    case "$name" in
      aos-mcp.capsule|\
      claude-pack.toml|codex-pack.toml|grok-pack.toml|\
      aos-oracle-plugins.tar.gz|runtime-compatibility.toml) ;;
      *) die "release checksum manifest names an unknown asset: $name" ;;
    esac
  done < "$names"
}

expected_blake3() {
  name=$1
  awk -v name="$name" '{ candidate = $2; sub(/^\.\//, "", candidate) } candidate == name { print $1; found = 1 } END { exit !found }' \
    "$RELEASE_STAGE/BLAKE3SUMS.txt"
}

verify_blake3() {
  path=$1
  name=$2
  expected=$(expected_blake3 "$name") \
    || die "release checksum manifest has no digest for $name"
  [ -n "$B3SUM" ] || return 0
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
  CURRENT_PACK_BINDINGS="$WORK/current-$host.bindings"
  pack_capsules_tsv "$pack" > "$CURRENT_PACK_BINDINGS"
  actual=$(awk '{print $1}' "$CURRENT_PACK_BINDINGS")
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
  stage_pack "$host"
  stage=$STAGED_PACK
  validate_pack "$host" "$principal" "$stage/Pack.toml"
  load_previous_bindings "$host" "$principal"
  OBSOLETE_BINDINGS="$WORK/obsolete-$host.bindings"
  : > "$OBSOLETE_BINDINGS"
  ensure_principal "$host" "$principal"

  for capsule in $(capsules_for "$host"); do
    expected_hash=$(binding_hash "$CURRENT_PACK_BINDINGS" "$capsule") \
      || die "signed pack has no managed hash for $capsule"
    previous_hash=$(binding_hash "$PREVIOUS_BINDINGS" "$capsule" 2>/dev/null || true)
    install_current=1
    if load_capsule_record "$principal" "$capsule"; then
      if [ -n "$previous_hash" ] && [ "$CAPSULE_HASH" != "$previous_hash" ]; then
        install_current=0
        say "Preserving locally superseded capsule '$capsule' for $principal."
      elif [ -z "$previous_hash" ] && [ "$CAPSULE_HASH" != "$expected_hash" ]; then
        install_current=0
        say "Preserving pre-existing capsule '$capsule' for $principal."
      elif [ -z "$previous_hash" ] && [ "$CAPSULE_HASH" = "$expected_hash" ]; then
        install_current=0
      fi
    fi

    if [ "$install_current" -eq 1 ]; then
      if [ "$ASSUME_YES" -eq 1 ]; then
        aos --principal "$principal" capsule install "$stage/$capsule.capsule" </dev/null
      elif [ -r /dev/tty ]; then
        aos --principal "$principal" capsule install "$stage/$capsule.capsule" </dev/tty
      else
        aos --principal "$principal" capsule install "$stage/$capsule.capsule"
      fi
      load_capsule_record "$principal" "$capsule" \
        || die "installed capsule '$capsule' has no readable identity"
      [ "$CAPSULE_HASH" = "$expected_hash" ] \
        || die "installed capsule '$capsule' does not match its signed pack identity"
    fi
  done

  set -- aos --principal default agent modify "$principal"
  for capsule in $(capsules_for "$host"); do
    set -- "$@" --add-capsule "$capsule"
  done
  "$@" >/dev/null

  while read -r previous_name previous_hash previous_extra; do
    [ -n "$previous_name" ] || continue
    [ -z "${previous_extra:-}" ] || die "invalid previous ownership state"
    if ! binding_hash "$CURRENT_PACK_BINDINGS" "$previous_name" >/dev/null 2>&1; then
      append_binding "$OBSOLETE_BINDINGS" "$previous_name" "$previous_hash"
    fi
  done < "$PREVIOUS_BINDINGS"
  say "✓ $host oracle capsules ready as $principal"
}

reconcile_obsolete_bindings() {
  ro_principal=$1
  ro_removals="$WORK/removals-${ro_principal}.txt"
  : > "$ro_removals"
  while read -r ro_name ro_hash ro_extra; do
    [ -n "$ro_name" ] || continue
    [ -z "${ro_extra:-}" ] || die "invalid obsolete ownership state"
    if ! load_capsule_record "$ro_principal" "$ro_name"; then
      printf '%s\n' "$ro_name" >> "$ro_removals"
    elif [ "$CAPSULE_HASH" = "$ro_hash" ]; then
      printf '%s\n' "$ro_name" >> "$ro_removals"
    else
      say "Preserving locally superseded capsule '$ro_name' for $ro_principal."
    fi
  done < "$OBSOLETE_BINDINGS"

  [ -s "$ro_removals" ] || return 0
  set -- aos --principal default agent modify "$ro_principal"
  while IFS= read -r ro_name; do
    set -- "$@" --remove-capsule "$ro_name"
  done < "$ro_removals"
  "$@" >/dev/null
  say "✓ obsolete Oracle capsule bindings reconciled for $ro_principal"
}

install_plugin() {
  host=$1
  case "$host" in
    claude)
      have claude || die "Claude Code is not installed"
      claude plugin marketplace remove unicity-aos-oracles >/dev/null 2>&1 || true
      claude plugin marketplace add "$PLUGIN_SNAPSHOT" >/dev/null
      claude plugin install unicity-aos@unicity-aos-oracles >/dev/null
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
      ;;
    grok)
      have grok || die "Grok Build is not installed"
      grok plugin install "$PLUGIN_SNAPSHOT/plugins/grok" --trust >/dev/null
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
  write_managed_capsules "$CURRENT_PACK_BINDINGS" "$stage/ManagedCapsules.toml"
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
hosts=$(select_hosts)
acquire_install_lock
ensure_aos
stage_release_metadata
if [ "$PLUGINS_ONLY" -eq 1 ]; then
  prepare_plugin_snapshot
  for host in $hosts; do
    install_plugin "$host"
  done
  say "Unicity AOS plugin installation complete. Start a new host session to provision its oracle pack."
  exit 0
fi
ensure_base
for host in $hosts; do
  install_pack "$host"
  prepare_plugin_snapshot
  if [ "$SKIP_HOST_PLUGIN" -eq 0 ]; then
    install_plugin "$host"
  fi
  reconcile_obsolete_bindings "$(principal_for "$host")"
  write_receipt "$host" "$(principal_for "$host")" "$STAGED_PACK"
done

say "Unicity AOS oracle installation complete. Start a new host session to load the plugin."
