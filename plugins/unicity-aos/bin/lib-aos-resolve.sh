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

aos_runtime_target() {
  _art_os=$(uname -s 2>/dev/null) || return 1
  _art_arch=$(uname -m 2>/dev/null) || return 1
  case "$_art_os/$_art_arch" in
    Darwin/arm64|Darwin/aarch64) printf 'aarch64-apple-darwin' ;;
    Darwin/x86_64) printf 'x86_64-apple-darwin' ;;
    Linux/aarch64|Linux/arm64) printf 'aarch64-unknown-linux-gnu' ;;
    Linux/x86_64|Linux/amd64) printf 'x86_64-unknown-linux-gnu' ;;
    *) return 1 ;;
  esac
}

_aos_verify_active_runtime_bytes() {
  [ -n "${ASTRID:-}" ] && [ -n "${_aos_expected_blake3:-}" ] \
    && [ -n "${_aos_expected_sha256:-}" ] || return 1
  [ -n "${_aos_active_runtime_path:-}" ] && [ "$ASTRID" = "$_aos_active_runtime_path" ] || {
    echo "aos-resolve: Astrid is not the canonical active release executable" >&2
    return 1
  }
  for _aos_checked_path in \
    "${_aos_home:-}" "${_aos_home:-}/releases" "${_aos_release:-}" \
    "${_aos_active_runtime_path%/*}" "${_aos_active_runtime_path%/*/*}"
  do
    [ -n "$_aos_checked_path" ] && [ -d "$_aos_checked_path" ] \
      && [ ! -L "$_aos_checked_path" ] || {
        echo "aos-resolve: active AOS release path is not a regular directory" >&2
        return 1
      }
  done
  if ! python3 - "${_aos_home:-}" <<'PY'
import os
import pathlib
import sys

path = pathlib.Path(os.path.abspath(sys.argv[1]))
if not path.is_absolute():
    raise SystemExit(1)
current = pathlib.Path(path.anchor)
for component in path.parts[1:]:
    current /= component
    if current.is_symlink() or not current.is_dir():
        raise SystemExit(1)
PY
  then
    echo "aos-resolve: AOS home has a symlinked ancestor" >&2
    return 1
  fi
  [ -f "$ASTRID" ] && [ ! -L "$ASTRID" ] && [ -x "$ASTRID" ] || {
    echo "aos-resolve: active AOS release is missing its bundled Astrid CLI" >&2
    return 1
  }
  command -v b3sum >/dev/null 2>&1 || {
    echo "aos-resolve: b3sum is required to authenticate the selected Astrid bytes" >&2
    return 127
  }
  if command -v sha256sum >/dev/null 2>&1; then
    _aos_actual_sha256=$(sha256sum "$ASTRID" 2>/dev/null | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    _aos_actual_sha256=$(shasum -a 256 "$ASTRID" 2>/dev/null | awk '{print $1}')
  else
    echo "aos-resolve: sha256sum or shasum is required to authenticate Astrid" >&2
    return 127
  fi
  _aos_actual_blake3=$(b3sum "$ASTRID" 2>/dev/null | awk '{print $1}')
  printf '%s\n' "$_aos_actual_blake3" | grep -Eq '^[0-9a-f]{64}$' \
    && [ "$_aos_actual_blake3" = "$_aos_expected_blake3" ] || {
      echo "aos-resolve: active Astrid BLAKE3 digest does not match its signed executable record" >&2
      return 1
    }
  printf '%s\n' "$_aos_actual_sha256" | grep -Eq '^[0-9a-f]{64}$' \
    && [ "$_aos_actual_sha256" = "$_aos_expected_sha256" ] || {
      echo "aos-resolve: active Astrid SHA-256 digest does not match its signed executable record" >&2
      return 1
    }
}

_aos_execute_active_runtime() {
  _aos_verify_active_runtime_bytes || return $?
  exec "$ASTRID" "$@"
}

# Resolve the Astrid CLI bundled in the authenticated active AOS release.
#
# The mutable runtime home is never an executable search path. The signed
# Oracle snapshot fixes the expected Astrid release, while AOS fixes the active
# product release. Both release receipts must agree, and the exact selected
# file must match its signed executable record immediately before execution.
aos_resolve_active_runtime() {
  aos_resolve_apply || return $?
  _aos_active_runtime_path=""
  _aos_expected_blake3=""
  _aos_expected_sha256=""
  _aos_home="${AOS_HOME:-$HOME/.aos}"
  _aos_plugin_root="$(_aos_resolve_plugin_root)"
  _aos_oracle_version_file="$_aos_plugin_root/.aos-oracle-version"
  [ -f "$_aos_oracle_version_file" ] && [ ! -L "$_aos_oracle_version_file" ] || {
    echo "aos-resolve: plugin snapshot has no regular Oracle release identity" >&2
    return 1
  }
  IFS= read -r _aos_oracle_version < "$_aos_oracle_version_file" || return 1
  printf '%s\n' "$_aos_oracle_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
    echo "aos-resolve: plugin snapshot has an invalid Oracle release identity" >&2
    return 1
  }
  _aos_version=$("$AOS" --version 2>/dev/null \
    | awk 'NF { value = $NF } END { print value }')
  printf '%s\n' "$_aos_version" \
    | grep -Eq '^20[0-9][0-9]\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || {
      echo "aos-resolve: active AOS command reported an invalid release version" >&2
      return 1
    }

  _aos_release="$_aos_home/releases/$_aos_version"
  _aos_runtime="$_aos_release/runtime/bin/astrid"
  _aos_release_statement="$_aos_release/unicity-aos-$_aos_version-release.toml"
  _aos_distro="$_aos_release/Distro.toml"
  _aos_manifest="$_aos_release/release-manifest.json"
  _aos_oracle_root="$_aos_home/extensions/oracles/codex"
  _aos_current="$_aos_oracle_root/current"
  _aos_receipt_release="$_aos_oracle_root/releases/$_aos_oracle_version"
  _aos_receipt="$_aos_current/Receipt.toml"
  _aos_pack="$_aos_current/Pack.lock"
  _aos_compat="$_aos_current/runtime-compatibility.toml"
  for _aos_path in \
    "$_aos_home" "$_aos_home/releases" "$_aos_release" \
    "$_aos_release/runtime" "$_aos_release/runtime/bin"
  do
    [ -d "$_aos_path" ] && [ ! -L "$_aos_path" ] || {
      echo "aos-resolve: active AOS release has an unsafe directory: $_aos_path" >&2
      return 1
    }
  done
  if ! python3 - "$_aos_home" <<'PY'
import os
import pathlib
import sys

path = pathlib.Path(os.path.abspath(sys.argv[1]))
current = pathlib.Path(path.anchor)
for component in path.parts[1:]:
    current /= component
    if current.is_symlink() or not current.is_dir():
        raise SystemExit(1)
PY
  then
    echo "aos-resolve: active AOS release has a symlinked home ancestor" >&2
    return 1
  fi
  [ -d "$_aos_oracle_root" ] && [ ! -L "$_aos_oracle_root" ] \
    && [ -d "$_aos_oracle_root/releases" ] \
    && [ ! -L "$_aos_oracle_root/releases" ] \
    && [ -d "$_aos_receipt_release" ] \
    && [ ! -L "$_aos_receipt_release" ] || {
      echo "aos-resolve: Oracle receipt directory is unsafe or missing" >&2
      return 1
    }
  [ -L "$_aos_current" ] \
    && [ "$(readlink "$_aos_current" 2>/dev/null)" = "releases/$_aos_oracle_version" ] || {
      echo "aos-resolve: active Oracle receipt does not select this plugin release" >&2
      return 1
    }
  for _aos_path in \
    "$_aos_distro" "$_aos_manifest" "$_aos_receipt" "$_aos_pack" "$_aos_compat"
  do
    [ -f "$_aos_path" ] && [ ! -L "$_aos_path" ] || {
      echo "aos-resolve: active AOS release is missing a regular receipt: $_aos_path" >&2
      return 1
    }
  done
  [ -f "$_aos_runtime" ] && [ ! -L "$_aos_runtime" ] && [ -x "$_aos_runtime" ] || {
    echo "aos-resolve: active AOS release is missing its bundled Astrid CLI" >&2
    return 1
  }
  [ -f "$_aos_release_statement" ] && [ ! -L "$_aos_release_statement" ] || {
    echo "aos-resolve: active AOS release has no regular signed executable statement" >&2
    return 1
  }
  grep -Fqx 'id = "unicity-ce"' "$_aos_distro" \
    && grep -Fqx "version = \"$_aos_version\"" "$_aos_distro" || {
      echo "aos-resolve: active AOS distribution identity does not match its release" >&2
      return 1
    }
  grep -Fqx "oracle-version = \"$_aos_oracle_version\"" "$_aos_receipt" \
    && grep -Fqx 'host = "codex"' "$_aos_receipt" \
    && grep -Fqx 'principal = "codex-code"' "$_aos_receipt" \
    && grep -Fqx "plugin-snapshot = \"../../../plugins/$_aos_oracle_version\"" "$_aos_receipt" \
    && grep -Fqx "version = \"$_aos_oracle_version\"" "$_aos_pack" || {
      echo "aos-resolve: active Oracle receipt identity does not match this plugin" >&2
      return 1
    }

  _aos_expected_runtime=0.11.0
  grep -Fqx 'repository = "astrid-runtime/astrid"' "$_aos_compat" \
    && grep -Fqx "version = \"$_aos_expected_runtime\"" "$_aos_compat" \
    && grep -Fqx "tag = \"v$_aos_expected_runtime\"" "$_aos_compat" \
    && grep -Fqx "version-requirement = \"=$_aos_expected_runtime\"" "$_aos_compat" \
    && grep -Fqx "release-workflow-identity = \"https://github.com/astrid-runtime/astrid/.github/workflows/release.yml@refs/tags/v$_aos_expected_runtime\"" "$_aos_compat" \
    && grep -Fqx 'release-ready = true' "$_aos_compat" || {
      echo "aos-resolve: Oracle receipt does not authorize a released Astrid $_aos_expected_runtime" >&2
      return 1
    }

  command -v python3 >/dev/null 2>&1 || {
    echo "aos-resolve: python3 is required to verify the active AOS release manifest" >&2
    return 127
  }

  _aos_runtime_target=$(aos_runtime_target) || {
    echo "aos-resolve: unsupported host platform for runtime authentication" >&2
    return 1
  }
  _aos_executable_digests=$(awk \
    -v product_version="$_aos_version" -v target="$_aos_runtime_target" '
    function valid_digest(value) {
      if (length(value) != 64) return 0
      return value ~ /^[0-9a-f]+$/
    }
    function valid_target(value) {
      return value == "aarch64-apple-darwin" || \
             value == "x86_64-apple-darwin" || \
             value == "aarch64-unknown-linux-gnu" || \
             value == "x86_64-unknown-linux-gnu"
    }
    function valid_path(value) {
      return value == "runtime/bin/astrid" || \
             value == "runtime/bin/astrid-daemon"
    }
    function finish_record() {
      if (!inside_record) return
      pair_key = record_target SUBSEP record_path
      if (record_target == "" || record_path == "" || !valid_digest(record_blake3) || !valid_digest(record_sha256) || fields != 4)
        fail("signed executable record has invalid fields or digests")
      if (!valid_target(record_target))
        fail("signed executable statement has an unsupported target")
      if (!valid_path(record_path))
        fail("signed executable statement has an unsupported executable path")
      if (pair_seen[pair_key]++)
        fail("duplicate executable record for target and path")
      total_records++
      if (record_target == target && record_path == "runtime/bin/astrid") {
        if (matched++) fail("duplicate executable record for this host and path")
        match_blake3 = record_blake3
        match_sha256 = record_sha256
      }
      inside_record = 0; fields = 0
      split("", seen)
      record_target = record_path = record_blake3 = record_sha256 = ""
    }
    function fail(message) {
      print "aos-resolve: " message > "/dev/stderr"
      exit 1
    }
    /^[[:space:]]*$/ || /^[[:space:]]*#/ { next }
    /^\[\[executables\]\]$/ {
      finish_record(); inside_record = 1; next
    }
    /^\[/ { finish_record(); next }
    inside_record {
      if ($0 !~ /^(target|path|blake3|sha256) = "[^"]*"$/) fail("invalid executable record field")
      key = $1; value = $0
      sub(/^[^"]*"/, "", value); sub(/"$/, "", value)
      if (seen[key SUBSEP inside_record]++) fail("duplicate executable record field")
      if (key == "target") record_target = value
      else if (key == "path") record_path = value
      else if (key == "blake3") record_blake3 = value
      else record_sha256 = value
      fields++; next
    }
    /^schema-version = 2$/ { schema = 1; next }
    /^product = "unicity-aos-ce"$/ { product = 1; next }
    {
      if ($0 ~ /^version = "[^"]*"$/) {
        value = $0; sub(/^version = "/, "", value); sub(/"$/, "", value)
        version = value
      }
      next
    }
    END {
      finish_record()
      if (!schema) fail("signed executable statement must use schema-version 2")
      if (!product || version != product_version) fail("signed executable statement product identity mismatch")
      if (total_records != 8) fail("signed executable statement must contain exactly eight executable records")
      delete unused_pair
      unused_pair["aarch64-apple-darwin" SUBSEP "runtime/bin/astrid"] = 1
      unused_pair["aarch64-apple-darwin" SUBSEP "runtime/bin/astrid-daemon"] = 1
      unused_pair["x86_64-apple-darwin" SUBSEP "runtime/bin/astrid"] = 1
      unused_pair["x86_64-apple-darwin" SUBSEP "runtime/bin/astrid-daemon"] = 1
      unused_pair["aarch64-unknown-linux-gnu" SUBSEP "runtime/bin/astrid"] = 1
      unused_pair["aarch64-unknown-linux-gnu" SUBSEP "runtime/bin/astrid-daemon"] = 1
      unused_pair["x86_64-unknown-linux-gnu" SUBSEP "runtime/bin/astrid"] = 1
      unused_pair["x86_64-unknown-linux-gnu" SUBSEP "runtime/bin/astrid-daemon"] = 1
      for (unused_key in unused_pair) {
        if (!(unused_key in pair_seen)) fail("signed executable statement is missing a GNU/Darwin executable record")
      }
      if (!matched) fail("signed executable statement does not authorize this Astrid path")
      print match_blake3, match_sha256
    }
    ' "$_aos_release_statement") || return 1
  read -r _aos_expected_blake3 _aos_expected_sha256 <<EOF
$_aos_executable_digests
EOF
  case "$_aos_executable_digests" in
    [0-9a-f]*' '[0-9a-f]*) ;;
    *) echo "aos-resolve: active AOS release lacks a unique signed Astrid executable record" >&2; return 1 ;;
  esac

  python3 - "$_aos_manifest" "$_aos_version" "$_aos_expected_runtime" <<'PY' || return 1
import json
import pathlib
import sys

path, product_version, runtime_version = sys.argv[1:]
try:
    manifest = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"aos-resolve: invalid active AOS release manifest: {error}")
if manifest.get("schema_version") != 2:
    raise SystemExit("aos-resolve: unsupported active AOS release manifest schema")
product = manifest.get("product")
runtime = manifest.get("runtime")
if (
    not isinstance(product, dict)
    or product.get("name") != "Unicity AOS Community Edition"
    or product.get("version") != product_version
):
    raise SystemExit("aos-resolve: active AOS manifest product version mismatch")
if manifest.get("layout") != {
    "release_directory": f"releases/{product_version}",
    "runtime_executables": "runtime/bin",
    "capsule_assets": "capsules",
}:
    raise SystemExit("aos-resolve: active AOS manifest runtime layout mismatch")
if not isinstance(runtime, dict):
    raise SystemExit("aos-resolve: active AOS manifest has no runtime identity")
if runtime.get("repository") != "astrid-runtime/astrid" or runtime.get("version") != runtime_version:
    raise SystemExit("aos-resolve: active AOS manifest runtime identity mismatch")
target = manifest.get("target")
if not isinstance(target, str) or not target:
    raise SystemExit("aos-resolve: active AOS manifest has no target identity")
if runtime.get("tag") != f"v{runtime_version}":
    raise SystemExit("aos-resolve: active AOS manifest runtime tag mismatch")
if runtime.get("asset") != f"astrid-{runtime_version}-{target}.tar.gz":
    raise SystemExit("aos-resolve: active AOS manifest runtime asset mismatch")
digest = runtime.get("digest")
if (
    not isinstance(digest, str)
    or not digest.startswith("blake3:")
    or len(digest) != len("blake3:") + 64
    or any(character not in "0123456789abcdef" for character in digest[7:])
):
    raise SystemExit("aos-resolve: active AOS manifest runtime digest is invalid")
expected_identity = (
    "https://github.com/astrid-runtime/astrid/.github/workflows/release.yml"
    f"@refs/tags/v{runtime_version}"
)
if runtime.get("release_workflow_identity") != expected_identity:
    raise SystemExit("aos-resolve: active AOS manifest runtime signer mismatch")
PY

  ASTRID="$_aos_runtime"
  ASTRID_RELEASE="$_aos_release"
  _aos_active_runtime_path="$_aos_runtime"
  export ASTRID ASTRID_RELEASE
  _aos_verify_active_runtime_bytes || return $?

  _aos_reported_runtime=$("$_aos_runtime" --version 2>/dev/null \
    | awk 'NF { value = $NF } END { print value }')
  [ "$_aos_reported_runtime" = "$_aos_expected_runtime" ] || {
    echo "aos-resolve: bundled Astrid reports $_aos_reported_runtime, expected $_aos_expected_runtime" >&2
    return 1
  }
}
