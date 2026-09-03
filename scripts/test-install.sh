#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
work=$(cd "$work" && pwd -P)
trap 'rm -rf "$work"' EXIT
fake_bin="$work/bin"
assets="$work/assets"
home="$work/home"
mkdir -p "$fake_bin" "$assets" "$home"
for host in claude codex grok; do
  cp "$repo_root/packs/$host.toml" "$assets/$host.toml"
done
cp "$repo_root/release/runtime-compatibility.toml" "$assets/runtime-compatibility.toml"
(cd "$repo_root" && tar -czf "$assets/aos-oracle-plugins.tar.gz" \
  .agents .claude-plugin .grok-plugin \
  plugins/claude plugins/grok plugins/unicity-aos)
product_assets="$work/product-assets"
mkdir -p "$product_assets/capsules"
printf '%s\n' \
  aos-cli.capsule \
  aos-mcp.capsule \
  aos-fs.capsule \
  aos-openai-compat.capsule \
  aos-skills.capsule \
  aos-forge.capsule > "$product_assets/capsule-assets.txt"
cat > "$product_assets/Distro.toml" <<'EOF'
schema-version = 1

[distro]
id = "unicity-ce"
version = "2026.9.0"

[[capsule]]
name = "aos-cli"
source = "capsules/aos-cli.capsule"

[[capsule]]
name = "aos-mcp"
source = "capsules/aos-mcp.capsule"

[[capsule]]
name = "aos-fs"
source = "capsules/aos-fs.capsule"

[[capsule]]
name = "aos-openai-compat"
source = "capsules/aos-openai-compat.capsule"

[[capsule]]
name = "aos-skills"
source = "capsules/aos-skills.capsule"

[[capsule]]
name = "aos-forge"
source = "capsules/aos-forge.capsule"
EOF
for capsule in aos-cli aos-mcp aos-fs aos-openai-compat aos-skills aos-forge; do
  printf 'signed product fixture for %s\n' "$capsule" \
    > "$product_assets/capsules/$capsule.capsule"
done

write_fixture_checksums() {
  root=$1
  : > "$root/BLAKE3SUMS.txt"
  for asset in \
    claude-pack.toml codex-pack.toml grok-pack.toml \
    aos-oracle-plugins.tar.gz runtime-compatibility.toml
  do
    source_name=$asset
    case "$asset" in
      *-pack.toml) source_name=${asset%-pack.toml}.toml ;;
    esac
    digest=$(shasum -a 256 "$root/$source_name" | awk '{print $1}')
    printf '%s  %s\n' "$digest" "$asset" >> "$root/BLAKE3SUMS.txt"
  done
}

write_fixture_checksums "$assets"

cat > "$fake_bin/aos" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = --version ]; then
  if [ -z "${TEST_NO_PRODUCT_INSTALL:-}" ]; then
    mkdir -p "$AOS_HOME/releases/2026.9.0"
    cp -R "$TEST_PRODUCT_ASSETS/." "$AOS_HOME/releases/2026.9.0/"
  fi
  printf 'Unicity AOS %s\n' "${TEST_AOS_VERSION:-2026.9.0}"
  exit 0
fi
printf 'aos' >> "$TEST_LOG"
printf ' %q' "$@" >> "$TEST_LOG"
printf '\n' >> "$TEST_LOG"
if [ -n "${AOS_VAR_AUTH_MODE:-}" ]; then
  printf 'claude-vars auth=%s interaction=%s api-key-set=%s\n' \
    "$AOS_VAR_AUTH_MODE" "${AOS_VAR_INTERACTION_MODE:-}" \
    "$([ -n "${AOS_VAR_API_KEY:-}" ] && printf yes || printf no)" >> "$TEST_LOG"
fi
if [ -n "${AOS_VAR_OPENAI_API_KEY:-}" ]; then
  printf 'openai-env api-key-set=yes\n' >> "$TEST_LOG"
fi
case " $* " in
  *" status "*)
    if [ -f "$AOS_HOME/runtime-running" ]; then
      printf '{"state":"running"}\n'
    else
      printf '{"state":"stopped"}\n'
    fi
    ;;
  *" ps --format json "*)
    if [ "${TEST_PS_FAILURE:-0}" -ne 0 ]; then
      printf '%s\n' 'error: workspace probe failed for another reason' >&2
      exit 1
    fi
    if [ "${TEST_STALE_WORKSPACE:-0}" -ne 0 ]; then
      printf '%s\n' 'error: running daemon belongs to another project or workspace layout' >&2
      exit 1
    fi
    printf '{}\n'
    ;;
  *" start "*)
    mkdir -p "$AOS_HOME"
    : > "$AOS_HOME/runtime-running"
    ;;
  *" stop "*)
    rm -f "$AOS_HOME/runtime-running"
    ;;
  *" agent show "*)
    principal=${*: -1}
    test -f "$TEST_STATE/agent-$principal"
    ;;
  *" group show "*)
    group=${*: -1}
    test -f "$TEST_STATE/group-$group"
    ;;
  *" group create "*)
    group=${5}
    : > "$TEST_STATE/group-$group"
    ;;
  *" agent create "*)
    principal=${5}
    : > "$TEST_STATE/agent-$principal"
    ;;
  *" capsule show "*)
    capsule=""
    principal=""
    previous=""
    for argument in "$@"; do
      if [ "$previous" = show ]; then capsule=$argument; fi
      if [ "$previous" = --agent ]; then principal=$argument; fi
      previous=$argument
    done
    record="$TEST_STATE/installed-$principal-$capsule"
    test -f "$record"
    if printf ' %s ' "$*" | grep -Fq ' --format toml '; then
      hash=$(sed -n '1p' "$record")
      source=$(sed -n '2p' "$record")
      installed=$(sed -n '3p' "$record")
      updated=$(sed -n '4p' "$record")
      printf 'name = "%s"\n' "$capsule"
      printf 'version = "0.1.0"\n'
      printf 'source = "%s"\n' "$source"
      printf 'wasm_hash = "%s"\n' "$hash"
      printf 'installed_at = "%s"\n' "$installed"
      printf 'updated_at = "%s"\n' "$updated"
    fi
    ;;
  *" capsule install "*)
    principal=default
    previous=""
    for argument in "$@"; do
      if [ "$previous" = --principal ]; then principal=$argument; break; fi
      previous=$argument
    done
    source=${*: -1}
    if [ "$source" = --yes ]; then source=${*: -2:1}; fi
    capsule=$(basename "$source" .capsule)
    case "$capsule" in
      aos-mcp) hash=a2e772db86cbbc1a19a86033254f9379a01fe2c07258bc419793316f9d40e95e ;;
      claude-install) hash=b5dd4e2beb234163419088187a87603a42284805de6e288b5450b712e24dfd2f ;;
      claude-runner) hash=19adab7d37a9be54a0a1866349594461f8116c65612134c124aae94fa79c3c63 ;;
      codex-install) hash=6c510fd2185311dd6de4fd44adb19f9ff19f2251adcad16ff18d859a434e8593 ;;
      codex-runner) hash=0b9473ccba844bce95fff41126c620107f71d630ee0e1d0dd23e5a542613642c ;;
      *) hash=$(shasum -a 256 "$source" | awk '{print $1}') ;;
    esac
    printf '%s\n%s\n%s\n%s\n' "$hash" "$source" \
      '2026-07-17T23:13:33+00:00' '2026-07-17T23:13:33+00:00' \
      > "$TEST_STATE/installed-$principal-$capsule"
    ;;
  *" agent modify "*)
    principal=${5}
    if [ "${TEST_SWAP_RELEASES:-0}" -eq 1 ]; then
      rm -rf "$AOS_HOME/extensions/oracles/codex/releases"
      ln -s "$TEST_SWAP_TARGET" "$AOS_HOME/extensions/oracles/codex/releases"
    fi
    previous=""
    for argument in "$@"; do
      case "$previous" in
        --add-capsule)
          : > "$TEST_STATE/granted-$principal-$argument"
          if [ ! -f "$TEST_STATE/installed-$principal-$argument" ] \
            && [ -f "$TEST_STATE/installed-default-$argument" ]; then
            cp "$TEST_STATE/installed-default-$argument" \
              "$TEST_STATE/installed-$principal-$argument"
          fi
          ;;
        --remove-capsule) rm -f "$TEST_STATE/granted-$principal-$argument" ;;
      esac
      previous=$argument
    done
    ;;
  *" init "*)
    version=${TEST_AOS_VERSION:-2026.9.0}
    release="$AOS_HOME/releases/$version"
    [ -f "$release/capsule-assets.txt" ] || exit 92
    while IFS= read -r asset; do
      capsule=${asset%.capsule}
      source="$release/capsules/$asset"
      hash=$(shasum -a 256 "$source" | awk '{print $1}')
      printf '%s\n%s\n%s\n%s\n' "$hash" "$source" \
        '2026-09-01T00:00:00+00:00' '2026-09-01T00:00:00+00:00' \
        > "$TEST_STATE/installed-default-$capsule"
    done < "$release/capsule-assets.txt"
    mkdir -p "$AOS_HOME"
    : > "$AOS_HOME/runtime-running"
    target=""
    previous=""
    for argument in "$@"; do
      if [ "$previous" = --target-principal ]; then target=$argument; break; fi
      previous=$argument
    done
    if [ -n "$target" ]; then
      : > "$TEST_STATE/product-$target"
    else
      : > "$TEST_STATE/default-initialized"
      mkdir -p "$AOS_HOME/runtime/etc/profiles"
      : > "$AOS_HOME/runtime/etc/profiles/default.toml"
    fi
    ;;
esac
EOF

cat > "$fake_bin/b3sum" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
digest=$(shasum -a 256 "$1" | awk '{print $1}')
printf '%s  %s\n' "$digest" "$1"
EOF

cat > "$fake_bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'codex' >> "$TEST_LOG"
printf ' %q' "$@" >> "$TEST_LOG"
printf '\n' >> "$TEST_LOG"
if [ -n "${TEST_PLUGIN_STATE:-}" ]; then
  mkdir -p "$(dirname "$TEST_PLUGIN_STATE")"
  : > "$TEST_PLUGIN_STATE"
fi
[ "${TEST_FAIL_PLUGIN:-0}" -eq 0 ] || exit 70
EOF
cat > "$fake_bin/claude" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'claude' >> "$TEST_LOG"
printf ' %q' "$@" >> "$TEST_LOG"
printf '\n' >> "$TEST_LOG"
[ "${TEST_FAIL_PLUGIN:-0}" -eq 0 ] || exit 70
EOF
cat > "$fake_bin/grok" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'grok' >> "$TEST_LOG"
printf ' %q' "$@" >> "$TEST_LOG"
printf '\n' >> "$TEST_LOG"
[ "${TEST_FAIL_PLUGIN:-0}" -eq 0 ] || exit 70
EOF
chmod +x \
  "$fake_bin/aos" "$fake_bin/b3sum" "$fake_bin/codex" "$fake_bin/claude" "$fake_bin/grok"

export PATH="$fake_bin:/usr/bin:/bin"
export AOS_HOME="$home/.aos"
export AOS_ORACLE_ASSETS="$assets"
export TEST_LOG="$work/commands.log"
export TEST_STATE="$work/state"
export TEST_PRODUCT_ASSETS="$product_assets"
mkdir -p "$TEST_STATE"
: > "$TEST_LOG"

write_test_capsule() {
  state=$1
  principal=$2
  name=$3
  hash=$4
  source=$5
  installed_at=$6
  updated_at=$7
  printf '%s\n%s\n%s\n%s\n' "$hash" "$source" "$installed_at" "$updated_at" \
    > "$state/installed-$principal-$name"
}

# A caller must not be able to turn the old per-capsule approval switch back
# into a global trust bypass. Both public installer surfaces recognize the flag
# as a rejected contract and fail before resolving installer bytes or creating
# durable AOS state.
approval_canary="$work/approval-installer"
: > "$approval_canary"
chmod 700 "$approval_canary"
approval_plugin_home="$home/approval-plugin/.aos"
if AOS_HOME="$approval_plugin_home" \
  AOS_ORACLES_INSTALLER="$approval_canary" \
  "$repo_root/plugins/unicity-aos/bin/aos-install" \
  --host codex --yes --approve-untrusted >"$work/approval-plugin.out" 2>&1
then
  echo "plugin installer accepted --approve-untrusted" >&2
  exit 1
fi
grep -Fq "aos-install: ERROR: --approve-untrusted is rejected: AOS dependencies require the signed OperatorDistribution" \
  "$work/approval-plugin.out"
test ! -s "$approval_canary"
test ! -e "$approval_plugin_home"

approval_root_home="$home/approval-root/.aos"
if AOS_HOME="$approval_root_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
  --approve-untrusted >"$work/approval-root.out" 2>&1
then
  echo "oracle installer accepted --approve-untrusted" >&2
  exit 1
fi
grep -Fq "aos-oracles: --approve-untrusted is rejected: AOS dependencies require the signed OperatorDistribution" \
  "$work/approval-root.out"
test ! -e "$approval_root_home"
test ! -e "$approval_root_home/runtime"
test ! -e "$approval_root_home/extensions/oracles/codex/Pack.lock"
test ! -s "$TEST_LOG"

# A value-taking option must not consume the rejected sentinel. Exercise the
# complete parser boundary before either surface can resolve installer bytes.
for sentinel_case in \
  '--aos-channel --approve-untrusted' \
  '--aos-version --approve-untrusted' \
  '--oracle-version --approve-untrusted' \
  '--aos-installer --approve-untrusted'
do
  # shellcheck disable=SC2086
  set -- $sentinel_case
  case_home="$home/parser-plugin-$1/.aos"
  if AOS_HOME="$case_home" AOS_ORACLES_INSTALLER="$approval_canary" \
    "$repo_root/plugins/unicity-aos/bin/aos-install" \
    --host codex --yes "$@" >"$work/parser-plugin.out" 2>&1
  then
    echo "plugin parser consumed sentinel in: $sentinel_case" >&2
    exit 1
  fi
  grep -Fq -- "--approve-untrusted is rejected" "$work/parser-plugin.out"
  test ! -s "$approval_canary"
  test ! -e "$case_home"

  root_case_home="$home/parser-root-$1/.aos"
  if AOS_HOME="$root_case_home" AOS_ORACLE_ASSETS="$assets" \
    "$repo_root/install.sh" --host codex --yes --no-install-aos "$@" \
    >"$work/parser-root.out" 2>&1
  then
    echo "root parser consumed sentinel in: $sentinel_case" >&2
    exit 1
  fi
  grep -Fq -- "--approve-untrusted is rejected" "$work/parser-root.out"
  test ! -e "$root_case_home"
  test ! -e "$root_case_home/runtime"
  test ! -e "$root_case_home/extensions/oracles/codex/Pack.lock"
  test ! -s "$TEST_LOG"
done

# A truncated option fails without shifting an unrelated later argument into
# its value or beginning provisioning.
for missing_option in --host --oracle-version --aos-channel --aos-version --local-assets --aos-installer; do
  missing_plugin_home="$home/missing-plugin-${missing_option#--}/.aos"
  if AOS_HOME="$missing_plugin_home" AOS_ORACLES_INSTALLER="$approval_canary" \
    "$repo_root/plugins/unicity-aos/bin/aos-install" \
    "$missing_option" >"$work/missing-plugin.out" 2>&1
  then
    echo "plugin parser accepted missing value for $missing_option" >&2
    exit 1
  fi
  test ! -s "$approval_canary"
  test ! -e "$missing_plugin_home"

  missing_root_home="$home/missing-root-${missing_option#--}/.aos"
  if AOS_HOME="$missing_root_home" AOS_ORACLE_ASSETS="$assets" \
    "$repo_root/install.sh" --host codex --yes --no-install-aos \
    "$missing_option" >"$work/missing-root.out" 2>&1
  then
    echo "root parser accepted missing value for $missing_option" >&2
    exit 1
  fi
  test ! -e "$missing_root_home"
  test ! -e "$missing_root_home/runtime"
  test ! -e "$missing_root_home/extensions/oracles/codex/Pack.lock"
  test ! -s "$TEST_LOG"
done

# The public one-command path installs only marketplace plugins. Host startup
# owns principal and capsule provisioning, so this path must not initialize or
# start AOS and must not create a pack receipt.
plugin_only_home="$home/plugins-only/.aos"
plugin_only_start=$(wc -l < "$TEST_LOG")
AOS_HOME="$plugin_only_home" \
  "$repo_root/install.sh" --plugins-only --host codex --yes --no-install-aos
tail -n "+$((plugin_only_start + 1))" "$TEST_LOG" > "$work/plugin-only.log"
grep -Eq '^codex plugin marketplace add /.*/plugin-stage$' "$work/plugin-only.log"
grep -Fq 'codex plugin add unicity-aos@unicity-aos-oracles' "$work/plugin-only.log"
if grep -Eq '^aos |^(claude|grok) ' "$work/plugin-only.log"; then
  echo "plugin-only installation provisioned AOS or another host" >&2
  exit 1
fi
test ! -e "$plugin_only_home/runtime"
test ! -e "$plugin_only_home/extensions/oracles/codex/Pack.lock"
test ! -e "$plugin_only_home/extensions/oracles/.install.lock"

# b3sum prefixes glob-expanded paths with ./ in release builds. The signed
# manifest parser accepts that exact producer form while still validating the
# normalized asset allowlist.
prefixed_assets="$work/prefixed-assets"
cp -R "$assets" "$prefixed_assets"
sed 's#  #  ./#' "$assets/BLAKE3SUMS.txt" > "$prefixed_assets/BLAKE3SUMS.txt"
prefixed_home="$home/prefixed-checksums/.aos"
AOS_HOME="$prefixed_home" AOS_ORACLE_ASSETS="$prefixed_assets" \
  "$repo_root/install.sh" --plugins-only --host codex --yes --no-install-aos
test -d "$prefixed_home/extensions/oracles/plugins/0.3.0"

# An existing unrelated host pack is private state. Installing Codex must not
# inspect, rewrite, remove, or provision Claude/Grok.
mkdir -p "$AOS_HOME/extensions/oracles/claude"
printf 'existing claude pack\n' > "$work/claude-before"
cp "$work/claude-before" "$AOS_HOME/extensions/oracles/claude/private-state"
codex_start=$(wc -l < "$TEST_LOG")

"$repo_root/install.sh" --host codex --yes --no-install-aos

tail -n "+$((codex_start + 1))" "$TEST_LOG" > "$work/codex-only.log"
cmp "$work/claude-before" "$AOS_HOME/extensions/oracles/claude/private-state"
if grep -Eq '^(claude|grok) ' "$work/codex-only.log" \
  || grep -Eq 'group (show|create) (claude|grok)' "$work/codex-only.log" \
  || grep -Eq 'agent (show|create|modify) (claude-code|grok-code)' "$work/codex-only.log"
then
  echo "Codex installation touched another oracle host" >&2
  exit 1
fi

lock="$AOS_HOME/extensions/oracles/codex/Pack.lock"
cmp "$assets/codex.toml" "$lock"
test ! -e "$home/.astrid"
test ! -e "$AOS_HOME/runtime/bin"
grep -Fq 'aos status --json' "$TEST_LOG"
grep -Fq 'aos --principal default init --yes' "$TEST_LOG"
[ "$(grep -Fc 'aos --principal default init --yes' "$TEST_LOG")" -eq 1 ]
if grep -Fq 'aos --principal default stop' "$TEST_LOG"; then
  echo "oracle installer stopped a runtime it does not exclusively own" >&2
  exit 1
fi
if grep -Fq 'aos --principal default status' "$TEST_LOG"; then
  echo "installer used the principal-scoped status probe" >&2
  exit 1
fi
if grep -Eq '^aos .* capsule install( |$)' "$TEST_LOG"; then
  echo "oracle host provisioning bypassed the signed operator distribution" >&2
  exit 1
fi
grep -Fq 'aos --principal default agent create codex-code' "$TEST_LOG"
grep -Fq 'aos --principal default agent create codex-code --group codex --yes' "$TEST_LOG"
if grep -Fq -- '--bare' "$TEST_LOG"; then
  echo "oracle principal used the unshipped per-agent distro bypass" >&2
  exit 1
fi
if grep -Fq -- '--inherit-from' "$TEST_LOG"; then
  echo "oracle principal inherited another principal's state" >&2
  exit 1
fi
grep -Fq -- '--add-capsule aos-mcp' "$TEST_LOG"
grep -Fq -- '--add-capsule aos-skills' "$TEST_LOG"
grep -Fq -- '--add-capsule aos-forge' "$TEST_LOG"
grep -Eq '^codex plugin marketplace add /.*/plugin-stage$' "$TEST_LOG"
grep -Fq 'codex plugin add unicity-aos@unicity-aos-oracles' "$TEST_LOG"
test -d "$AOS_HOME/extensions/oracles/plugins/0.3.0"
test -L "$AOS_HOME/extensions/oracles/codex/current"
test -f "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
test -f "$AOS_HOME/extensions/oracles/codex/current/ManagedCapsules.toml"
grep -Fq 'source = "local"' "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
test "$(cat "$AOS_HOME/extensions/oracles/codex/current/ManagedCapsules.toml")" \
  = 'schema-version = 1'
test -f "$TEST_STATE/installed-codex-code-aos-mcp"
test -f "$TEST_STATE/installed-codex-code-aos-skills"
test -f "$TEST_STATE/installed-codex-code-aos-forge"
test -f "$TEST_STATE/granted-codex-code-aos-mcp"
test -f "$TEST_STATE/granted-codex-code-aos-skills"
test -f "$TEST_STATE/granted-codex-code-aos-forge"
test ! -e "$AOS_HOME/extensions/oracles/.install.lock"

# AOS-owned optional services are resolved from the active signed product
# release. Older compatible AOS releases can omit Forge while the generic
# skills index remains required and granted.
without_forge_assets="$work/product-without-forge"
without_forge_state="$work/state-without-forge"
without_forge_home="$home/without-forge/.aos"
mkdir -p "$without_forge_state"
cp -R "$product_assets" "$without_forge_assets"
rm "$without_forge_assets/capsules/aos-forge.capsule"
grep -Fvx 'aos-forge.capsule' "$product_assets/capsule-assets.txt" \
  > "$without_forge_assets/capsule-assets.txt"
grep -Fv 'aos-forge' "$product_assets/Distro.toml" \
  > "$without_forge_assets/Distro.toml"
without_forge_start=$(wc -l < "$TEST_LOG")
TEST_STATE="$without_forge_state" TEST_PRODUCT_ASSETS="$without_forge_assets" \
  AOS_HOME="$without_forge_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
tail -n "+$((without_forge_start + 1))" "$TEST_LOG" > "$work/without-forge.log"
grep -Fq -- '--add-capsule aos-skills' "$work/without-forge.log"
if grep -Fq -- '--add-capsule aos-forge' "$work/without-forge.log"; then
  echo "optional Forge was granted when the active AOS release did not ship it" >&2
  exit 1
fi
test -f "$without_forge_state/installed-codex-code-aos-skills"
test ! -e "$without_forge_state/installed-codex-code-aos-forge"

without_skills_assets="$work/product-without-skills"
without_skills_state="$work/state-without-skills"
without_skills_home="$home/without-skills/.aos"
mkdir -p "$without_skills_state"
cp -R "$product_assets" "$without_skills_assets"
rm "$without_skills_assets/capsules/aos-skills.capsule"
grep -Fvx 'aos-skills.capsule' "$product_assets/capsule-assets.txt" \
  > "$without_skills_assets/capsule-assets.txt"
grep -Fv 'aos-skills' "$product_assets/Distro.toml" \
  > "$without_skills_assets/Distro.toml"
without_skills_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$without_skills_state" TEST_PRODUCT_ASSETS="$without_skills_assets" \
  AOS_HOME="$without_skills_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "host pack accepted an AOS release without its required skills service" >&2
  exit 1
fi
tail -n "+$((without_skills_start + 1))" "$TEST_LOG" > "$work/without-skills.log"
if grep -Eq '^aos .* init( |$)|capsule install .*/aos-mcp\.capsule|^codex ' "$work/without-skills.log"; then
  echo "required AOS dependency failure mutated the Oracle pack or host plugin" >&2
  exit 1
fi
test ! -e "$without_skills_home/extensions/oracles/codex/current"

# A same-ID user capsule is not a valid substitute for the signed AOS service.
# Reject the transaction before the immutable Oracle receipt or a grant lands.
local_skills_state="$work/state-local-skills"
local_skills_home="$home/local-skills/.aos"
mkdir -p "$local_skills_state"
write_test_capsule "$local_skills_state" codex-code aos-skills \
  aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  /tmp/user/aos-skills.capsule \
  2026-07-19T12:00:00+00:00 2026-07-19T12:00:00+00:00
local_skills_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$local_skills_state" AOS_HOME="$local_skills_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "same-ID foreign AOS capsule was accepted as operator distribution state" >&2
  exit 1
fi
tail -n "+$((local_skills_start + 1))" "$TEST_LOG" > "$work/local-skills.log"
if grep -Fq -- '--add-capsule aos-skills' "$work/local-skills.log"; then
  echo "same-ID local skills capsule was auto-granted as an AOS dependency" >&2
  exit 1
fi
if grep -Fq 'aos --principal default init --yes' "$work/local-skills.log"; then
  echo "same-ID foreign AOS capsule triggered operator-distribution mutation" >&2
  exit 1
fi
test ! -e "$local_skills_state/granted-codex-code-aos-skills"
test ! -e "$local_skills_home/extensions/oracles/codex/current"
test ! -e "$local_skills_state/default-initialized"
test ! -e "$local_skills_state/agent-codex-code"
test ! -e "$local_skills_state/installed-codex-code-aos-mcp"
test ! -e "$local_skills_home/runtime"
test ! -e "$local_skills_home/runtime/etc/profiles/default.toml"
test ! -e "$local_skills_home/extensions/oracles/plugins/0.3.0"
test "$(sed -n '2p' "$local_skills_state/installed-codex-code-aos-skills")" \
  = /tmp/user/aos-skills.capsule

# A malformed installed identity is not the same as an absent capsule. It must
# stop before workspace selection or default first-boot can mutate AOS state.
malformed_identity_state="$work/malformed-identity-state"
malformed_identity_home="$home/malformed-identity/.aos"
malformed_identity_start=$(wc -l < "$TEST_LOG")
mkdir -p "$malformed_identity_state"
write_test_capsule "$malformed_identity_state" default aos-skills \
  zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz \
  "$product_assets/capsules/aos-skills.capsule" \
  2026-09-03T00:00:00+00:00 2026-09-03T00:00:00+00:00
if TEST_STATE="$malformed_identity_state" AOS_HOME="$malformed_identity_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
  >"$work/malformed-identity-direct.out" 2>&1
then
  echo "malformed default AOS capsule identity was treated as absent" >&2
  exit 1
fi
grep -Fq 'has a malformed identity' "$work/malformed-identity-direct.out"
tail -n "+$((malformed_identity_start + 1))" "$TEST_LOG" \
  > "$work/malformed-identity.log"
test ! -e "$malformed_identity_state/default-initialized"
test ! -e "$malformed_identity_state/agent-codex-code"
test ! -e "$malformed_identity_state/granted-codex-code-aos-skills"
test ! -e "$malformed_identity_home/runtime"
test ! -e "$malformed_identity_home/extensions/oracles/plugins/0.3.0"
test ! -e "$malformed_identity_home/extensions/oracles/codex/current"

# A source can look correct while its recorded byte identity disagrees with the
# authenticated default. That disagreement stops the transaction before init,
# principal creation, grants, snapshot activation, or any receipt state.
identity_mismatch_home="$home/identity-mismatch/.aos"
identity_mismatch_state="$work/identity-mismatch-state"
identity_release="$identity_mismatch_home/releases/2026.9.0"
identity_artifact="$identity_release/capsules/aos-mcp.capsule"
mkdir -p "$identity_mismatch_state" "$identity_release/capsules"
cp -R "$product_assets/." "$identity_release/"
printf 'release artifact\n' > "$identity_artifact"
write_test_capsule "$identity_mismatch_state" default aos-mcp \
  1111111111111111111111111111111111111111111111111111111111111111 \
  "$identity_artifact" \
  2026-09-03T00:00:00+00:00 2026-09-03T00:00:00+00:00
write_test_capsule "$identity_mismatch_state" codex-code aos-mcp \
  2222222222222222222222222222222222222222222222222222222222222222 \
  "$identity_artifact" \
  2026-09-03T00:00:00+00:00 2026-09-03T00:00:00+00:00
identity_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$identity_mismatch_state" AOS_HOME="$identity_mismatch_home" \
  TEST_NO_PRODUCT_INSTALL=1 \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "disagreeing default and host capsule hashes were accepted" >&2
  exit 1
fi
tail -n "+$((identity_start + 1))" "$TEST_LOG" > "$work/identity-mismatch.log"
if grep -Fq 'aos --principal default init --yes' "$work/identity-mismatch.log"; then
  echo "identity disagreement triggered first-boot mutation" >&2
  exit 1
fi
test ! -e "$identity_mismatch_state/default-initialized"
test ! -e "$identity_mismatch_state/agent-codex-code"
test ! -e "$identity_mismatch_state/granted-codex-code-aos-mcp"
test ! -e "$identity_mismatch_home/runtime"
test ! -e "$identity_mismatch_home/extensions/oracles/plugins/0.3.0"
test ! -e "$identity_mismatch_home/extensions/oracles/codex/current"

# Local development may stage only the selected host, provided every staged
# byte has a strict checksum entry.
minimal_assets="$work/minimal-assets"
mkdir -p "$minimal_assets"
for asset in \
  aos-oracle-plugins.tar.gz runtime-compatibility.toml codex.toml
do
  cp "$assets/$asset" "$minimal_assets/$asset"
done
: > "$minimal_assets/BLAKE3SUMS.txt"
for asset in \
  codex-pack.toml \
  aos-oracle-plugins.tar.gz runtime-compatibility.toml
do
  source_name=$asset
  case "$asset" in
    codex-pack.toml) source_name=codex.toml ;;
  esac
  digest=$(shasum -a 256 "$minimal_assets/$source_name" | awk '{print $1}')
  printf '%s  %s\n' "$digest" "$asset" >> "$minimal_assets/BLAKE3SUMS.txt"
done
minimal_home="$home/minimal/.aos"
minimal_state="$work/minimal-state"
mkdir -p "$minimal_state"
TEST_STATE="$minimal_state" AOS_HOME="$minimal_home" AOS_ORACLE_ASSETS="$minimal_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
test -f "$minimal_home/extensions/oracles/codex/Pack.lock"

first_lock=$(shasum -a 256 "$lock" | awk '{print $1}')
init_count=$(grep -Fc 'aos --principal default init --yes' "$TEST_LOG" || true)
"$repo_root/install.sh" --host codex --yes --no-install-aos
test "$first_lock" = "$(shasum -a 256 "$lock" | awk '{print $1}')"
test "$(grep -Fc 'aos --principal default init --yes' "$TEST_LOG" || true)" -eq "$init_count"
if grep -Fq 'aos --principal default stop' "$TEST_LOG"; then
  echo "repeat oracle install stopped the shared runtime" >&2
  exit 1
fi

# A daemon selected by an older host plugin from another project is stopped
# through the recovery command and restarted in the product-owned workspace.
stale_home="$home/stale-workspace/.aos"
stale_state="$work/stale-workspace-state"
mkdir -p "$stale_state"
mkdir -p "$stale_home"
: > "$stale_home/runtime-running"
stale_start=$(wc -l < "$TEST_LOG")
TEST_STATE="$stale_state" AOS_HOME="$stale_home" TEST_STALE_WORKSPACE=1 \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
tail -n "+$((stale_start + 1))" "$TEST_LOG" > "$work/stale-workspace.log"
grep -Fq 'aos --principal default ps --format json' "$work/stale-workspace.log"
grep -Fq 'aos --principal default stop' "$work/stale-workspace.log"
grep -Fq 'aos --principal default init --yes' "$work/stale-workspace.log"

failed_probe_home="$home/failed-workspace-probe/.aos"
mkdir -p "$failed_probe_home"
: > "$failed_probe_home/runtime-running"
failed_probe_start=$(wc -l < "$TEST_LOG")
if AOS_HOME="$failed_probe_home" TEST_PS_FAILURE=1 \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "unrelated runtime probe failure was treated as a workspace mismatch" >&2
  exit 1
fi
tail -n "+$((failed_probe_start + 1))" "$TEST_LOG" > "$work/failed-workspace-probe.log"
if grep -Fq 'aos --principal default stop' "$work/failed-workspace-probe.log"; then
  echo "unrelated runtime probe failure stopped the runtime" >&2
  exit 1
fi

distribution=$(grep -n 'aos --principal default init --yes' "$TEST_LOG" | head -n1 | cut -d: -f1)
create=$(grep -n 'agent create codex-code' "$TEST_LOG" | head -n1 | cut -d: -f1)
grant=$(grep -n 'agent modify codex-code' "$TEST_LOG" | head -n1 | cut -d: -f1)
test "$distribution" -lt "$create"
test "$create" -lt "$grant"

# Grok is a separate pack. Installing it provisions only grok-code, the Oracle
# broker, and the signed pack's selected AOS services; it installs its own host
# plugin, writes its own receipt, and leaves legacy Astrid plugin state alone.
legacy_grok="$home/.grok/plugins/astrid/private-state"
mkdir -p "$(dirname "$legacy_grok")"
printf 'legacy grok plugin state\n' > "$work/grok-before"
cp "$work/grok-before" "$legacy_grok"
grok_start=$(wc -l < "$TEST_LOG")
"$repo_root/install.sh" --host grok --yes --no-install-aos
tail -n "+$((grok_start + 1))" "$TEST_LOG" > "$work/grok-only.log"
cmp "$work/grok-before" "$legacy_grok"
if grep -Eq '^aos .* (init|capsule install)( |$)' "$work/grok-only.log"; then
  echo "Grok provisioning re-applied or bypassed the operator distribution" >&2
  exit 1
fi
grep -Fq -- 'agent modify grok-code --add-capsule aos-mcp' "$work/grok-only.log"
grep -Fq -- '--add-capsule aos-skills' "$work/grok-only.log"
grep -Fq -- '--add-capsule aos-forge' "$work/grok-only.log"
grep -Eq '^grok plugin install .*/plugins/grok --trust$' "$work/grok-only.log"
if grep -Eq '^(codex|claude) ' "$work/grok-only.log"; then
  echo "Grok installation touched another oracle host" >&2
  exit 1
fi
grok_receipt="$AOS_HOME/extensions/oracles/grok/current/Receipt.toml"
test -f "$grok_receipt"
grep -Fq 'host = "grok"' "$grok_receipt"
grep -Fq 'principal = "grok-code"' "$grok_receipt"

# A host plugin uses the host application's existing authentication. Installing
# the external Claude plugin must not require or consume an Anthropic API key.
claude_start=$(wc -l < "$TEST_LOG")
env -u ANTHROPIC_API_KEY \
  "$repo_root/install.sh" --host claude --yes --no-install-aos
test -f "$AOS_HOME/extensions/oracles/claude/Pack.lock"
tail -n "+$((claude_start + 1))" "$TEST_LOG" > "$work/claude-only.log"
if grep -Eq '^aos .* (init|capsule install)( |$)' "$work/claude-only.log"; then
  echo "Claude provisioning re-applied or bypassed the operator distribution" >&2
  exit 1
fi
grep -Fq -- 'agent modify claude-code --add-capsule aos-mcp' "$work/claude-only.log"
grep -Fq -- '--add-capsule aos-skills' "$work/claude-only.log"
grep -Fq -- '--add-capsule aos-forge' "$work/claude-only.log"
grep -Fq 'claude plugin install unicity-aos@unicity-aos-oracles' "$TEST_LOG"
grep -Eq '^claude plugin marketplace add /.*/plugin-stage$' "$TEST_LOG"
if grep -Eq 'capsule install .*/claude-(install|runner)\.capsule' "$work/claude-only.log"; then
  echo "external Claude plugin installed an AOS-managed workload adapter" >&2
  exit 1
fi
if grep -Fq 'claude-vars ' "$work/claude-only.log"; then
  echo "external Claude plugin consumed workload authentication variables" >&2
  exit 1
fi

# A plugin failure leaves no success receipt for a fresh installation.
failed_plugin_home="$home/plugin-failure/.aos"
failed_plugin_home_state="$failed_plugin_home/test-state"
failed_plugin_state="$failed_plugin_home_state"
failed_plugin_marketplace="$failed_plugin_home/fake-host-marketplace"
if TEST_FAIL_PLUGIN=1 TEST_STATE="$failed_plugin_state" \
  TEST_PLUGIN_STATE="$failed_plugin_marketplace" AOS_HOME="$failed_plugin_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "oracle install unexpectedly succeeded after plugin failure" >&2
  exit 1
fi
test ! -e "$failed_plugin_home"
test ! -e "$failed_plugin_state/default-initialized"
test ! -e "$failed_plugin_state/agent-codex-code"
test ! -e "$failed_plugin_state/granted-codex-code-aos-mcp"
test ! -e "$failed_plugin_home/extensions/oracles/codex/Pack.lock"
test ! -e "$failed_plugin_home/extensions/oracles/codex/current"
test ! -e "$failed_plugin_home/extensions/oracles/.install.lock"

# A release-directory substitution after the early preflight fails receipt
# commit, writes nothing to the substituted destination, and rolls back the
# entire fresh AOS transaction.
swap_home="$home/release-swap/.aos"
swap_state="$swap_home/test-state"
swap_outside="$work/release-swap-outside"
mkdir -p "$swap_outside"
printf 'not Oracle trust state\n' > "$swap_outside/receipt"
if TEST_SWAP_RELEASES=1 TEST_SWAP_TARGET="$swap_outside" \
  TEST_STATE="$swap_state" AOS_HOME="$swap_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "swapped receipt release root unexpectedly committed" >&2
  exit 1
fi
test ! -e "$swap_home"
test ! -e "$swap_state/default-initialized"
test ! -e "$swap_state/agent-codex-code"
test ! -e "$swap_state/granted-codex-code-aos-mcp"
test "$(cat "$swap_outside/receipt")" = 'not Oracle trust state'
test ! -e "$swap_outside/Receipt.toml"

# A failed host-plugin replacement cannot advance an already authenticated
# Oracle generation. Its immutable receipt and selected current generation
# remain byte-for-byte available for a later clean retry.
prior_plugin_home="$home/plugin-failure-prior/.aos"
prior_plugin_state="$work/plugin-failure-prior-state"
prior_receipt="$prior_plugin_home/extensions/oracles/codex/releases/0.2.6"
mkdir -p "$prior_plugin_state" "$prior_receipt"
cat > "$prior_receipt/Pack.lock" <<'EOF'
schema-version = 1

[pack]
version = "0.2.6"
host = "codex"
principal = "codex-code"
EOF
cat > "$prior_receipt/Receipt.toml" <<'EOF'
schema-version = 1
oracle-version = "0.2.6"
host = "codex"
principal = "codex-code"
source = "release"
EOF
printf 'authenticated prior generation\n' > "$prior_receipt/prior-marker"
ln -s releases/0.2.6 "$prior_plugin_home/extensions/oracles/codex/current"
ln -s current/Pack.lock "$prior_plugin_home/extensions/oracles/codex/Pack.lock"
prior_receipt_hash=$(shasum -a 256 "$prior_receipt/Receipt.toml" | awk '{print $1}')
if TEST_FAIL_PLUGIN=1 TEST_STATE="$prior_plugin_state" AOS_HOME="$prior_plugin_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "oracle replacement unexpectedly succeeded after plugin failure" >&2
  exit 1
fi
test "$(readlink "$prior_plugin_home/extensions/oracles/codex/current")" = releases/0.2.6
test ! -e "$prior_plugin_home/extensions/oracles/codex/releases/0.3.0"
test "$(shasum -a 256 "$prior_receipt/Receipt.toml" | awk '{print $1}')" \
  = "$prior_receipt_hash"
grep -Fxq 'authenticated prior generation' "$prior_receipt/prior-marker"

# Local development assets cannot inherit a Sigstore bundle from an older
# remote receipt.
stale_bundle="$AOS_HOME/extensions/oracles/codex/Pack.lock.sigstore.json"
rm -f "$stale_bundle"
printf 'stale\n' > "$stale_bundle"
"$repo_root/install.sh" --host codex --yes --no-install-aos
test ! -e "$stale_bundle"

# A signed pack's product-version floor is enforced before any capsule from
# that pack is installed or its receipt is written.
incompatible_home="$home/incompatible/.aos"
incompatible_start=$(wc -l < "$TEST_LOG")
if TEST_AOS_VERSION=2025.9.0 AOS_HOME="$incompatible_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "pack unexpectedly installed on an incompatible AOS version" >&2
  exit 1
fi
tail -n "+$((incompatible_start + 1))" "$TEST_LOG" > "$work/incompatible.log"
if grep -Fq 'capsule install' "$work/incompatible.log"; then
  echo "incompatible pack installed a capsule" >&2
  exit 1
fi
test ! -e "$incompatible_home/extensions/oracles/codex/Pack.lock"

# An exact product version request cannot silently settle on another version
# that merely satisfies the pack floor.
noop_installer="$work/aos-installer.sh"
printf '%s\n' '#!/usr/bin/env sh' 'exit 0' > "$noop_installer"
exact_home="$home/exact-version/.aos"
if TEST_AOS_VERSION=2026.1.1 AOS_HOME="$exact_home" \
  AOS_INSTALL_URL="file://$noop_installer" \
  "$repo_root/install.sh" --host codex --yes --aos-version 2026.2.0
then
  echo "exact AOS version mismatch unexpectedly succeeded" >&2
  exit 1
fi
test ! -e "$exact_home/extensions/oracles/.install.lock"

# The signed checksum manifest is enforced for every staged pack asset.
tampered_assets="$work/tampered-assets"
mkdir -p "$tampered_assets"
cp -R "$assets/." "$tampered_assets/"
printf 'tampered\n' >> "$tampered_assets/codex.toml"
tampered_home="$home/tampered/.aos"
if AOS_HOME="$tampered_home" AOS_ORACLE_ASSETS="$tampered_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "checksum-mismatched pack unexpectedly installed" >&2
  exit 1
fi
test ! -e "$tampered_home/extensions/oracles/codex/Pack.lock"

# Link entries are rejected before an archive can become an installed snapshot.
unsafe_assets="$work/unsafe-assets"
unsafe_tree="$work/unsafe-tree"
mkdir -p "$unsafe_assets" "$unsafe_tree"
cp -R "$assets/." "$unsafe_assets/"
tar -xzf "$unsafe_assets/aos-oracle-plugins.tar.gz" -C "$unsafe_tree"
ln -s /etc/passwd "$unsafe_tree/plugins/unicity-aos/unsafe-link"
tar -czf "$unsafe_assets/aos-oracle-plugins.tar.gz" -C "$unsafe_tree" .
write_fixture_checksums "$unsafe_assets"
unsafe_home="$home/unsafe-archive/.aos"
if AOS_HOME="$unsafe_home" AOS_ORACLE_ASSETS="$unsafe_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "symlink-bearing plugin archive unexpectedly installed" >&2
  exit 1
fi
test ! -e "$unsafe_home/extensions/oracles/codex/Pack.lock"

hardlink_assets="$work/hardlink-assets"
hardlink_tree="$work/hardlink-tree"
mkdir -p "$hardlink_assets" "$hardlink_tree"
cp -R "$assets/." "$hardlink_assets/"
tar -xzf "$hardlink_assets/aos-oracle-plugins.tar.gz" -C "$hardlink_tree"
ln "$hardlink_tree/plugins/unicity-aos/.mcp.json" \
  "$hardlink_tree/plugins/unicity-aos/hardlink-entry"
tar -czf "$hardlink_assets/aos-oracle-plugins.tar.gz" -C "$hardlink_tree" .
write_fixture_checksums "$hardlink_assets"
if AOS_HOME="$home/hardlink-archive/.aos" AOS_ORACLE_ASSETS="$hardlink_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "hardlink-bearing plugin archive unexpectedly installed" >&2
  exit 1
fi

special_assets="$work/special-assets"
special_tree="$work/special-tree"
mkdir -p "$special_assets" "$special_tree"
cp -R "$assets/." "$special_assets/"
tar -xzf "$special_assets/aos-oracle-plugins.tar.gz" -C "$special_tree"
mkfifo "$special_tree/plugins/unicity-aos/special-entry"
COPYFILE_DISABLE=1 tar -czf "$special_assets/aos-oracle-plugins.tar.gz" \
  -C "$special_tree" .
write_fixture_checksums "$special_assets"
if AOS_HOME="$home/special-archive/.aos" AOS_ORACLE_ASSETS="$special_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "special-entry plugin archive unexpectedly installed" >&2
  exit 1
fi

# Destination containment is checked before release staging or AOS mutation. A
# symlinked plugin ancestor cannot turn extraction or rename into an escape.
aos_link_base="$home/destination-aos-base"
aos_link_real="$home/destination-aos-real"
plugin_link_home="$aos_link_base/aos"
plugin_escape="$home/destination-plugin-escape"
mkdir "$aos_link_real" "$plugin_escape"
ln -s "$aos_link_real" "$aos_link_base"
destination_state="$work/destination-aos-link-state"
mkdir -p "$destination_state"
aos_destination_log_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$destination_state" AOS_HOME="$plugin_link_home" \
  AOS_ORACLE_ASSETS="$assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
  >"$work/destination-aos.out" 2>&1
then
  echo "symlinked AOS home ancestor was accepted" >&2
  exit 1
fi
grep -Fq "AOS home contains a symlink or non-directory: $aos_link_base" \
  "$work/destination-aos.out"
test ! -e "$aos_link_real/aos"
test "$(wc -l < "$TEST_LOG")" -eq "$aos_destination_log_start"

plugin_link_home="$home/destination-plugin-link/.aos"
plugin_escape="$home/destination-plugin-escape"
mkdir -p "$plugin_link_home/extensions/oracles" "$plugin_escape"
ln -s "$plugin_escape/plugins" "$plugin_link_home/extensions/oracles/plugins"
destination_state="$work/destination-link-state"
mkdir -p "$destination_state"
plugin_destination_log_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$destination_state" AOS_HOME="$plugin_link_home" \
  AOS_ORACLE_ASSETS="$assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
  >"$work/destination-plugin.out" 2>&1
then
  echo "symlinked plugin ancestor was accepted" >&2
  exit 1
fi
grep -Fq "plugin snapshot root contains a symlink or non-directory" \
  "$work/destination-plugin.out"
test ! -e "$plugin_escape/plugins"
test "$(wc -l < "$TEST_LOG")" -eq "$plugin_destination_log_start"

# The immutable receipt destination is held with the same rule, even when the
# intermediate releases directory is real but the version leaf is a link.
receipt_link_home="$home/destination-receipt-link/.aos"
receipt_escape="$home/destination-receipt-escape"
mkdir -p "$receipt_link_home/extensions/oracles/codex/releases" "$receipt_escape"
ln -s "$receipt_escape/0.3.0" "$receipt_link_home/extensions/oracles/codex/releases/0.3.0"
destination_state="$work/destination-receipt-state"
mkdir -p "$destination_state"
receipt_destination_log_start=$(wc -l < "$TEST_LOG")
if TEST_STATE="$destination_state" AOS_HOME="$receipt_link_home" \
  AOS_ORACLE_ASSETS="$assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
  >"$work/destination-receipt.out" 2>&1
then
  echo "symlinked receipt destination was accepted" >&2
  exit 1
fi
grep -Fq "codex receipt destination is a symlink" "$work/destination-receipt.out"
test ! -e "$receipt_escape/0.3.0"
test "$(wc -l < "$TEST_LOG")" -eq "$receipt_destination_log_start"

# A released version directory is immutable. Reruns may reuse identical bytes,
# but must not replace a snapshot or receipt that differs.
immutable_home="$home/immutable/.aos"
immutable_state="$work/immutable-state"
mkdir -p "$immutable_state"
TEST_STATE="$immutable_state" AOS_HOME="$immutable_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
snapshot_manifest="$immutable_home/extensions/oracles/plugins/0.3.0/.agents/plugins/marketplace.json"
printf '\nmodified\n' >> "$snapshot_manifest"
if TEST_STATE="$immutable_state" AOS_HOME="$immutable_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "modified immutable plugin snapshot was replaced" >&2
  exit 1
fi
grep -Fq modified "$snapshot_manifest"

receipt_home="$home/immutable-receipt/.aos"
receipt_state="$work/immutable-receipt-state"
mkdir -p "$receipt_state"
TEST_STATE="$receipt_state" AOS_HOME="$receipt_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
receipt="$receipt_home/extensions/oracles/codex/releases/0.3.0/Receipt.toml"
printf '\nmodified = true\n' >> "$receipt"
if TEST_STATE="$receipt_state" AOS_HOME="$receipt_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "modified immutable receipt was replaced" >&2
  exit 1
fi
grep -Fq 'modified = true' "$receipt"

# v0.2.0 provisioned a CE distro into each selected host principal before it
# installed the Oracle capsules. The v0.2.2 repair detaches only bindings whose
# exact identity is attributable to that transaction. Installed capsule files
# remain in place, unrelated grants survive, and same-ID local replacements are
# preserved before the new pack is staged.
upgrade_assets="$work/upgrade-assets"
cp -R "$assets" "$upgrade_assets"
write_fixture_checksums "$upgrade_assets"

legacy_home="$home/legacy-v020/.aos"
legacy_state="$work/legacy-state"
legacy_log="$work/legacy.log"
legacy_receipt="$legacy_home/extensions/oracles/codex/releases/0.2.0"
mkdir -p "$legacy_state" "$legacy_receipt" \
  "$legacy_home/releases/2026.1.1/capsules"
: > "$legacy_log"
: > "$legacy_home/runtime-running"
: > "$legacy_state/group-codex"
: > "$legacy_state/agent-codex-code"
cat > "$legacy_receipt/Receipt.toml" <<'EOF'
schema-version = 1
oracle-version = "0.2.0"
host = "codex"
principal = "codex-code"
source = "release"
EOF
cat > "$legacy_receipt/Pack.lock" <<'EOF'
schema-version = 1

[pack]
id = "codex-oracle"
name = "Unicity AOS for Codex"
version = "0.2.0"
host = "codex"
principal = "codex-code"
description = "Codex integration for Unicity AOS."
repository = "https://github.com/unicity-aos/oracles"
license = "MIT OR Apache-2.0"
aos-version = ">=2026.1.0"

[[capsule]]
name = "aos-mcp"
asset = "aos-mcp.capsule"

[[capsule]]
name = "codex-install"
asset = "codex-install.capsule"

[[capsule]]
name = "codex-runner"
asset = "codex-runner.capsule"
EOF
ln -s releases/0.2.0 "$legacy_home/extensions/oracles/codex/current"
ln -s current/Pack.lock "$legacy_home/extensions/oracles/codex/Pack.lock"
cat > "$legacy_home/releases/2026.1.1/Distro.toml" <<'EOF'
schema-version = 1

[distro]
id = "unicity-ce"
version = "2026.1.1"

[[capsule]]
name = "aos-cli"
source = "capsules/aos-cli.capsule"

[[capsule]]
name = "aos-fs"
source = "capsules/aos-fs.capsule"

[[capsule]]
name = "aos-skills"
source = "capsules/aos-skills.capsule"

[[capsule]]
name = "aos-forge"
source = "capsules/aos-forge.capsule"
EOF
: > "$legacy_home/releases/2026.1.1/capsules/aos-cli.capsule"
: > "$legacy_home/releases/2026.1.1/capsules/aos-fs.capsule"
: > "$legacy_home/releases/2026.1.1/capsules/aos-skills.capsule"
: > "$legacy_home/releases/2026.1.1/capsules/aos-forge.capsule"
printf '%s\n' \
  aos-cli.capsule \
  aos-fs.capsule \
  aos-skills.capsule \
  aos-forge.capsule \
  > "$legacy_home/releases/2026.1.1/capsule-assets.txt"

product_mcp_hash=$(shasum -a 256 "$product_assets/capsules/aos-mcp.capsule" | awk '{print $1}')
write_test_capsule "$legacy_state" codex-code aos-mcp \
  "$product_mcp_hash" \
  "$legacy_home/releases/2026.9.0/capsules/aos-mcp.capsule" \
  2026-07-17T23:13:33+00:00 2026-07-17T23:14:00+00:00
write_test_capsule "$legacy_state" codex-code codex-install \
  6c510fd2185311dd6de4fd44adb19f9ff19f2251adcad16ff18d859a434e8593 \
  /tmp/v0.2.0/codex-install.capsule \
  2026-07-17T23:13:33+00:00 2026-07-17T23:13:33+00:00
write_test_capsule "$legacy_state" codex-code codex-runner \
  eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee \
  /tmp/user/codex-runner.capsule \
  2026-07-17T23:13:33+00:00 2026-07-17T23:14:00+00:00
write_test_capsule "$legacy_state" codex-code aos-cli \
  dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd \
  "$legacy_home/releases/2026.1.1/capsules/aos-cli.capsule" \
  2026-07-17T23:12:00+00:00 2026-07-17T23:12:00+00:00
write_test_capsule "$legacy_state" codex-code aos-fs \
  cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc \
  "$legacy_home/releases/2026.1.1/capsules/aos-fs.capsule" \
  2026-07-17T23:12:00+00:00 2026-07-18T00:00:00+00:00
write_test_capsule "$legacy_state" codex-code user-capsule \
  bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  /tmp/user/user-capsule.capsule \
  2026-07-17T20:00:00+00:00 2026-07-17T20:00:00+00:00
for capsule in aos-mcp codex-install codex-runner aos-cli aos-fs user-capsule; do
  : > "$legacy_state/granted-codex-code-$capsule"
done

if TEST_FAIL_PLUGIN=1 TEST_STATE="$legacy_state" TEST_LOG="$legacy_log" \
  TEST_AOS_VERSION=2026.9.0 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.3.0
then
  echo "legacy repair unexpectedly committed after host plugin failure" >&2
  exit 1
fi
test -f "$legacy_state/granted-codex-code-codex-install"
test -f "$legacy_state/granted-codex-code-aos-cli"
test ! -e "$legacy_home/extensions/oracles/codex/releases/0.3.0"

TEST_STATE="$legacy_state" TEST_LOG="$legacy_log" \
  TEST_AOS_VERSION=2026.9.0 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.3.0
test ! -e "$legacy_state/granted-codex-code-codex-install"
test ! -e "$legacy_state/granted-codex-code-aos-cli"
test -f "$legacy_state/granted-codex-code-codex-runner"
test -f "$legacy_state/granted-codex-code-aos-fs"
test -f "$legacy_state/granted-codex-code-user-capsule"
test -f "$legacy_state/granted-codex-code-aos-mcp"
test "$(sed -n '1p' "$legacy_state/installed-codex-code-aos-mcp")" \
  = "$product_mcp_hash"
test -f "$legacy_state/installed-codex-code-codex-install"
test -f "$legacy_home/extensions/oracles/codex/releases/0.3.0/ManagedCapsules.toml"
test "$(cat "$legacy_home/extensions/oracles/codex/releases/0.3.0/ManagedCapsules.toml")" \
  = 'schema-version = 1'
if grep -Eq 'codex-(install|runner)|aos-(cli|fs)' \
  "$legacy_home/extensions/oracles/codex/releases/0.3.0/ManagedCapsules.toml"
then
  echo "new Oracle receipt claimed an obsolete or CE capsule" >&2
  exit 1
fi

# The immutable current pack receipt remains stable when the user keeps a
# same-ID superseding implementation.
receipt_before=$(shasum -a 256 \
  "$legacy_home/extensions/oracles/codex/releases/0.3.0/ManagedCapsules.toml" \
  | awk '{print $1}')
TEST_STATE="$legacy_state" TEST_LOG="$legacy_log" \
  TEST_AOS_VERSION=2026.9.0 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.3.0
test "$receipt_before" = "$(shasum -a 256 \
  "$legacy_home/extensions/oracles/codex/releases/0.3.0/ManagedCapsules.toml" \
  | awk '{print $1}')"
test "$(sed -n '1p' "$legacy_state/installed-codex-code-aos-mcp")" \
  = "$product_mcp_hash"

# A live per-home lock fails closed and an unsuccessful contender never removes
# the active installer's lock.
locked_home="$home/locked/.aos"
locked_state="$work/locked-state"
mkdir -p "$locked_state"
mkdir -p "$locked_home/extensions/oracles"
lock_path="$locked_home/extensions/oracles/.install.lock"
lock_ready="$work/live-lock-ready"
lock_release="$work/live-lock-release"
(
  exec 9>>"$lock_path"
  if command -v flock >/dev/null 2>&1; then
    flock -n 9
  else
    lockf -s -t 0 9
  fi
  : > "$lock_ready"
  while [ ! -e "$lock_release" ]; do sleep 0.01; done
) &
live_lock_pid=$!
while [ ! -e "$lock_ready" ]; do sleep 0.01; done
if TEST_STATE="$locked_state" AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "installer lock without a published diagnostic pid was ignored" >&2
  : > "$lock_release"
  wait "$live_lock_pid"
  exit 1
fi
test ! -s "$lock_path"
printf '%s\n' "$live_lock_pid" > "$lock_path"
if TEST_STATE="$locked_state" AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "concurrent installer lock was ignored" >&2
  kill "$live_lock_pid" 2>/dev/null || true
  exit 1
fi
test "$(cat "$lock_path")" = "$live_lock_pid"
: > "$lock_release"
wait "$live_lock_pid"

# A lock whose validated owner no longer exists is reclaimed atomically.
printf '%s\n' 999999999 > "$lock_path"
TEST_STATE="$locked_state" AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
test ! -e "$locked_home/extensions/oracles/.install.lock"

# Missing and malformed stale lock files are reclaimed by the platform lock.
for abandoned in missing malformed; do
  abandoned_home="$home/abandoned-$abandoned/.aos"
  abandoned_state="$work/abandoned-$abandoned-state"
  mkdir -p "$abandoned_state"
  mkdir -p "$abandoned_home/extensions/oracles"
  : > "$abandoned_home/extensions/oracles/.install.lock"
  if [ "$abandoned" = malformed ]; then
    printf '%s\n' not-a-pid > "$abandoned_home/extensions/oracles/.install.lock"
  fi
  TEST_STATE="$abandoned_state" AOS_HOME="$abandoned_home" \
    "$repo_root/install.sh" --host codex --yes --no-install-aos
  test ! -e "$abandoned_home/extensions/oracles/.install.lock"
  test -f "$abandoned_home/extensions/oracles/codex/Pack.lock"
done

python3 "$repo_root/scripts/test_release_contract.py"
