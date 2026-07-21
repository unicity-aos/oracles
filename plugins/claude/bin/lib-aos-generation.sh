# Exact AOS/runtime generation fencing shared by host plugin launchers.
#
# A plugin snapshot may bootstrap only a missing host pack. It must never
# replace a different committed pack or connect that snapshot to another AOS
# runtime generation.

# shellcheck shell=sh

AOS_GENERATION_ERROR=""
AOS_SNAPSHOT_RUNTIME_GENERATION=""

aos_snapshot_generation_load() {
  _aos_generation_plugin_root=$1
  _aos_generation_snapshot="$_aos_generation_plugin_root/.aos-runtime-generation"
  if [ ! -f "$_aos_generation_snapshot" ] || [ -L "$_aos_generation_snapshot" ]; then
    AOS_GENERATION_ERROR="plugin snapshot has no exact AOS runtime generation; reinstall the host plugin"
    return 1
  fi
  IFS= read -r AOS_SNAPSHOT_RUNTIME_GENERATION < "$_aos_generation_snapshot" \
    || AOS_SNAPSHOT_RUNTIME_GENERATION=""
  if ! printf '%s\n' "$AOS_SNAPSHOT_RUNTIME_GENERATION" \
    | grep -Eq '^astrid:(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*):[0-9a-f]{40}$'
  then
    AOS_GENERATION_ERROR="plugin snapshot has an invalid AOS runtime generation; reinstall the host plugin"
    return 1
  fi
}

# Print missing, current, upgrade, or stale. A snapshot may advance an older
# committed pack, but an older snapshot can never reactivate a newer pack.
aos_semver_at_least() {
  _aos_generation_candidate=$1
  _aos_generation_floor=$2
  awk -v candidate="$_aos_generation_candidate" -v floor="$_aos_generation_floor" 'BEGIN {
    split(candidate, c, ".")
    split(floor, f, ".")
    ok = (c[1] > f[1]) ||
         (c[1] == f[1] && c[2] > f[2]) ||
         (c[1] == f[1] && c[2] == f[2] && c[3] >= f[3])
    exit !ok
  }'
}

aos_oracle_pack_version() {
  _aos_generation_receipt=$1
  awk '
    $1 == "version" {
      count++
      if ($2 != "=" || NF != 3 || $3 !~ /^"[0-9]+\.[0-9]+\.[0-9]+"$/) bad = 1
      value = $3
      sub(/^"/, "", value)
      sub(/"$/, "", value)
    }
    END {
      if (bad || count != 1) exit 1
      print value
    }
  ' "$_aos_generation_receipt"
}

aos_oracle_pack_state() {
  _aos_generation_receipt=$1
  _aos_generation_oracle_version=$2
  if [ ! -e "$_aos_generation_receipt" ] && [ ! -L "$_aos_generation_receipt" ]; then
    printf '%s\n' missing
  else
    if [ -L "$_aos_generation_receipt" ]; then
      _aos_generation_target=$(readlink "$_aos_generation_receipt" 2>/dev/null || true)
      case "$_aos_generation_target" in
        current/Pack.lock|current/generation/Pack.lock) ;;
        *) printf '%s\n' stale; return ;;
      esac
    fi
    if [ ! -f "$_aos_generation_receipt" ]; then
      printf '%s\n' stale
      return
    fi
    _aos_generation_installed=$(aos_oracle_pack_version "$_aos_generation_receipt") || {
      printf '%s\n' stale
      return
    }
    if [ "$_aos_generation_installed" = "$_aos_generation_oracle_version" ]; then
      printf '%s\n' current
    elif aos_semver_at_least \
      "$_aos_generation_oracle_version" "$_aos_generation_installed"
    then
      printf '%s\n' upgrade
    else
      printf '%s\n' stale
    fi
  fi
}

aos_active_runtime_generation() {
  _aos_generation_marker=$1
  awk '
    $1 == "runtime-generation" {
      count++
      if ($2 != "=" || NF != 3 || $3 !~ /^"astrid:[0-9]+\.[0-9]+\.[0-9]+:[0-9a-f]{40}"$/) bad = 1
      value = $3
      sub(/^"/, "", value)
      sub(/"$/, "", value)
    }
    END {
      if (bad || count != 1) exit 1
      print value
    }
  ' "$_aos_generation_marker"
}

aos_runtime_generation_matches() {
  _aos_generation_home=$1
  _aos_generation_active="$_aos_generation_home/update/active-generation.toml"
  if [ ! -f "$_aos_generation_active" ] || [ -L "$_aos_generation_active" ]; then
    AOS_GENERATION_ERROR="installed AOS has no committed runtime generation; update or reinstall AOS, then restart this host"
    return 1
  fi
  _aos_generation_actual=$(aos_active_runtime_generation "$_aos_generation_active") || {
    AOS_GENERATION_ERROR="installed AOS has an invalid runtime generation marker; reinstall AOS, then restart this host"
    return 1
  }
  if [ "$_aos_generation_actual" != "$AOS_SNAPSHOT_RUNTIME_GENERATION" ]; then
    AOS_GENERATION_ERROR="plugin runtime $AOS_SNAPSHOT_RUNTIME_GENERATION does not match installed AOS runtime $_aos_generation_actual; update the host plugin and restart this host"
    return 1
  fi
}

aos_expected_host_generation() {
  _aos_generation_host=$1
  _aos_generation_oracle_version=$2
  printf 'oracle:%s:%s:%s\n' \
    "$_aos_generation_host" \
    "$_aos_generation_oracle_version" \
    "$AOS_SNAPSHOT_RUNTIME_GENERATION"
}

aos_oracle_marker_matches() {
  _aos_generation_marker=$1
  _aos_generation_expected=$2
  if [ ! -f "$_aos_generation_marker" ]; then
    return 2
  fi
  if [ -L "$_aos_generation_marker" ]; then
    _aos_generation_target=$(readlink "$_aos_generation_marker" 2>/dev/null || true)
    [ "$_aos_generation_target" = current/generation/Generation.lock ] || {
      AOS_GENERATION_ERROR="active Oracle generation marker has an unsafe target; reinstall this host pack, then restart this host"
      return 1
    }
  fi
  if ! awk -v expected="$_aos_generation_expected" '
    { if (NR != 1 || $0 != expected) bad = 1 }
    END { exit !(NR == 1 && !bad) }
  ' "$_aos_generation_marker"
  then
    AOS_GENERATION_ERROR="active Oracle generation does not match this plugin snapshot; update the host plugin, then restart this host"
    return 1
  fi
}

# Validate host state without starting AOS. A missing or older pack returns 2
# so this newer snapshot may deliberately bootstrap/upgrade it; stale or
# mixed-generation state returns 1 and sets AOS_GENERATION_ERROR.
aos_oracle_generation_preflight() {
  _aos_generation_receipt=$1
  _aos_generation_oracle_version=$2
  _aos_generation_home=$3
  _aos_generation_state=$(aos_oracle_pack_state \
    "$_aos_generation_receipt" "$_aos_generation_oracle_version")
  case "$_aos_generation_state" in
    missing|upgrade)
      _aos_generation_active="$_aos_generation_home/update/active-generation.toml"
      if [ -e "$_aos_generation_active" ] || [ -L "$_aos_generation_active" ]; then
        aos_runtime_generation_matches "$_aos_generation_home" || return 1
      elif [ -e "$_aos_generation_home/bin/aos" ] \
        || [ -L "$_aos_generation_home/bin/aos" ]
      then
        AOS_GENERATION_ERROR="installed AOS has no committed runtime generation; update or reinstall AOS before provisioning this host pack"
        return 1
      fi
      return 2
      ;;
    stale)
      AOS_GENERATION_ERROR="installed oracle pack is newer or invalid for plugin release $_aos_generation_oracle_version; update the host plugin, then restart this host"
      return 1
      ;;
    current)
      aos_runtime_generation_matches "$_aos_generation_home" || return 1
      _aos_generation_root=${_aos_generation_receipt%/Pack.lock}
      _aos_generation_host=${_aos_generation_root##*/}
      _aos_generation_expected=$(aos_expected_host_generation \
        "$_aos_generation_host" "$_aos_generation_oracle_version")
      aos_oracle_marker_matches \
        "$_aos_generation_root/Generation.lock" "$_aos_generation_expected"
      ;;
  esac
}
