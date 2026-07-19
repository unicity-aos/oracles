#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fake_bin="$work/bin"
assets="$work/assets"
home="$work/home"
mkdir -p "$fake_bin" "$assets" "$home"
for host in claude codex grok; do
  cp "$repo_root/packs/$host.toml" "$assets/$host.toml"
  printf '%s\n' \
    'wasm-blake3 = "a2e772db86cbbc1a19a86033254f9379a01fe2c07258bc419793316f9d40e95e"' \
    >> "$assets/$host.toml"
done
cp "$repo_root/release/runtime-compatibility.toml" "$assets/runtime-compatibility.toml"
(cd "$repo_root" && tar -czf "$assets/aos-oracle-plugins.tar.gz" \
  .agents .claude-plugin .grok-plugin \
  plugins/claude plugins/grok plugins/unicity-aos)
printf 'signed fixture for aos-mcp\n' > "$assets/aos-mcp.capsule"

product_assets="$work/product-assets"
mkdir -p "$product_assets/capsules"
printf '%s\n' \
  aos-cli.capsule \
  aos-fs.capsule \
  aos-openai-compat.capsule \
  aos-skills.capsule \
  aos-forge.capsule > "$product_assets/capsule-assets.txt"
cat > "$product_assets/Distro.toml" <<'EOF'
schema-version = 1

[distro]
id = "unicity-ce"
version = "2026.1.0"

[[capsule]]
name = "aos-cli"
source = "capsules/aos-cli.capsule"

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
for capsule in aos-cli aos-fs aos-openai-compat aos-skills aos-forge; do
  printf 'signed product fixture for %s\n' "$capsule" \
    > "$product_assets/capsules/$capsule.capsule"
done

write_fixture_checksums() {
  root=$1
  : > "$root/BLAKE3SUMS.txt"
  for asset in \
    aos-mcp.capsule \
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
  mkdir -p "$AOS_HOME/releases/2026.1.0"
  cp -R "$TEST_PRODUCT_ASSETS/." "$AOS_HOME/releases/2026.1.0/"
  printf 'Unicity AOS %s\n' "${TEST_AOS_VERSION:-2026.1.0}"
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
    previous=""
    for argument in "$@"; do
      case "$previous" in
        --add-capsule) : > "$TEST_STATE/granted-$principal-$argument" ;;
        --remove-capsule) rm -f "$TEST_STATE/granted-$principal-$argument" ;;
      esac
      previous=$argument
    done
    ;;
  *" init "*)
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

# The public one-command path installs only marketplace plugins. Host startup
# owns principal and capsule provisioning, so this path must not initialize or
# start AOS and must not create a pack receipt.
plugin_only_home="$home/plugins-only/.aos"
plugin_only_start=$(wc -l < "$TEST_LOG")
AOS_HOME="$plugin_only_home" \
  "$repo_root/install.sh" --plugins-only --host codex --yes --no-install-aos
tail -n "+$((plugin_only_start + 1))" "$TEST_LOG" > "$work/plugin-only.log"
grep -Fq "codex plugin marketplace add $plugin_only_home/extensions/oracles/plugins/0.2.5" \
  "$work/plugin-only.log"
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
test -d "$prefixed_home/extensions/oracles/plugins/0.2.5"

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
grep -Fq 'aos status --json' "$TEST_LOG"
grep -Fq 'aos --principal default start' "$TEST_LOG"
if grep -Fq 'aos --principal default stop' "$TEST_LOG"; then
  echo "oracle installer stopped a runtime it does not exclusively own" >&2
  exit 1
fi
if grep -Fq 'aos --principal default status' "$TEST_LOG"; then
  echo "installer used the principal-scoped status probe" >&2
  exit 1
fi
if grep -Eq '^aos .* init( |$)' "$TEST_LOG"; then
  echo "oracle host provisioning initialized a default product workspace" >&2
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
grep -Eq 'capsule install .*/aos-mcp\.capsule' "$TEST_LOG"
grep -Eq 'capsule install .*/releases/2026\.1\.0/capsules/aos-skills\.capsule --yes$' "$TEST_LOG"
grep -Eq 'capsule install .*/releases/2026\.1\.0/capsules/aos-forge\.capsule --yes$' "$TEST_LOG"
grep -Fq -- '--add-capsule aos-mcp' "$TEST_LOG"
grep -Fq -- '--add-capsule aos-skills' "$TEST_LOG"
grep -Fq -- '--add-capsule aos-forge' "$TEST_LOG"
if grep -Eq '^aos --principal codex-code capsule install .*/aos-(cli|fs|openai-compat)\.capsule' "$TEST_LOG"; then
  echo "oracle host provisioning installed an undeclared CE distribution capsule" >&2
  exit 1
fi
grep -Fq "codex plugin marketplace add $AOS_HOME/extensions/oracles/plugins/0.2.5" "$TEST_LOG"
grep -Fq 'codex plugin add unicity-aos@unicity-aos-oracles' "$TEST_LOG"
test -L "$AOS_HOME/extensions/oracles/codex/current"
test -f "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
test -f "$AOS_HOME/extensions/oracles/codex/current/ManagedCapsules.toml"
grep -Fq 'source = "local"' "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
grep -Fq 'name = "aos-mcp"' \
  "$AOS_HOME/extensions/oracles/codex/current/ManagedCapsules.toml"
grep -Fq 'wasm-hash = "a2e772db86cbbc1a19a86033254f9379a01fe2c07258bc419793316f9d40e95e"' \
  "$AOS_HOME/extensions/oracles/codex/current/ManagedCapsules.toml"
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
if grep -Eq 'capsule install .*/aos-mcp\.capsule|^codex ' "$work/without-skills.log"; then
  echo "required AOS dependency failure mutated the Oracle pack or host plugin" >&2
  exit 1
fi
test ! -e "$without_skills_home/extensions/oracles/codex/current"

# A same-ID user capsule is not a valid substitute for the signed AOS service.
# Preserve it, but do not have the Oracle installer manufacture a new grant.
local_skills_state="$work/state-local-skills"
local_skills_home="$home/local-skills/.aos"
mkdir -p "$local_skills_state"
write_test_capsule "$local_skills_state" codex-code aos-skills \
  aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  /tmp/user/aos-skills.capsule \
  2026-07-19T12:00:00+00:00 2026-07-19T12:00:00+00:00
local_skills_start=$(wc -l < "$TEST_LOG")
TEST_STATE="$local_skills_state" AOS_HOME="$local_skills_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
tail -n "+$((local_skills_start + 1))" "$TEST_LOG" > "$work/local-skills.log"
if grep -Fq -- '--add-capsule aos-skills' "$work/local-skills.log"; then
  echo "same-ID local skills capsule was auto-granted as an AOS dependency" >&2
  exit 1
fi
test ! -e "$local_skills_state/granted-codex-code-aos-skills"
test "$(sed -n '2p' "$local_skills_state/installed-codex-code-aos-skills")" \
  = /tmp/user/aos-skills.capsule

# Local development may stage only the selected host, provided every staged
# byte has a strict checksum entry.
minimal_assets="$work/minimal-assets"
mkdir -p "$minimal_assets"
for asset in \
  aos-oracle-plugins.tar.gz runtime-compatibility.toml codex.toml aos-mcp.capsule
do
  cp "$assets/$asset" "$minimal_assets/$asset"
done
: > "$minimal_assets/BLAKE3SUMS.txt"
for asset in \
  aos-mcp.capsule codex-pack.toml \
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
AOS_HOME="$minimal_home" AOS_ORACLE_ASSETS="$minimal_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
test -f "$minimal_home/extensions/oracles/codex/Pack.lock"

first_lock=$(shasum -a 256 "$lock" | awk '{print $1}')
start_count=$(grep -Fc 'aos --principal default start' "$TEST_LOG")
"$repo_root/install.sh" --host codex --yes --no-install-aos
test "$first_lock" = "$(shasum -a 256 "$lock" | awk '{print $1}')"
test "$(grep -Fc 'aos --principal default start' "$TEST_LOG")" -eq "$start_count"
if grep -Fq 'aos --principal default stop' "$TEST_LOG"; then
  echo "repeat oracle install stopped the shared runtime" >&2
  exit 1
fi

# A daemon selected by an older host plugin from another project is stopped
# through the recovery command and restarted in the product-owned workspace.
stale_home="$home/stale-workspace/.aos"
mkdir -p "$stale_home"
: > "$stale_home/runtime-running"
stale_start=$(wc -l < "$TEST_LOG")
AOS_HOME="$stale_home" TEST_STALE_WORKSPACE=1 \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
tail -n "+$((stale_start + 1))" "$TEST_LOG" > "$work/stale-workspace.log"
grep -Fq 'aos --principal default ps --format json' "$work/stale-workspace.log"
grep -Fq 'aos --principal default stop' "$work/stale-workspace.log"
grep -Fq 'aos --principal default start' "$work/stale-workspace.log"

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

create=$(grep -n 'agent create codex-code' "$TEST_LOG" | head -n1 | cut -d: -f1)
first_install=$(grep -n 'aos --principal codex-code capsule install' "$TEST_LOG" | head -n1 | cut -d: -f1)
test "$create" -lt "$first_install"

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
grep -Eq 'capsule install .*/aos-mcp\.capsule$' "$work/grok-only.log"
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
grep -Eq 'capsule install .*/aos-mcp\.capsule$' "$work/claude-only.log"
grep -Fq -- 'agent modify claude-code --add-capsule aos-mcp' "$work/claude-only.log"
grep -Fq -- '--add-capsule aos-skills' "$work/claude-only.log"
grep -Fq -- '--add-capsule aos-forge' "$work/claude-only.log"
grep -Fq 'claude plugin install unicity-aos@unicity-aos-oracles' "$TEST_LOG"
grep -Fq "claude plugin marketplace add $AOS_HOME/extensions/oracles/plugins/0.2.5" "$TEST_LOG"
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
if TEST_FAIL_PLUGIN=1 AOS_HOME="$failed_plugin_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "oracle install unexpectedly succeeded after plugin failure" >&2
  exit 1
fi
test ! -e "$failed_plugin_home/extensions/oracles/codex/Pack.lock"
test ! -e "$failed_plugin_home/extensions/oracles/codex/current"
test ! -e "$failed_plugin_home/extensions/oracles/.install.lock"

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
printf 'tampered\n' >> "$tampered_assets/aos-mcp.capsule"
tampered_home="$home/tampered/.aos"
if AOS_HOME="$tampered_home" AOS_ORACLE_ASSETS="$tampered_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "checksum-mismatched capsule unexpectedly installed" >&2
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

# A released version directory is immutable. Reruns may reuse identical bytes,
# but must not replace a snapshot or receipt that differs.
immutable_home="$home/immutable/.aos"
AOS_HOME="$immutable_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
snapshot_manifest="$immutable_home/extensions/oracles/plugins/0.2.5/.agents/plugins/marketplace.json"
printf '\nmodified\n' >> "$snapshot_manifest"
if AOS_HOME="$immutable_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "modified immutable plugin snapshot was replaced" >&2
  exit 1
fi
grep -Fq modified "$snapshot_manifest"

receipt_home="$home/immutable-receipt/.aos"
AOS_HOME="$receipt_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
receipt="$receipt_home/extensions/oracles/codex/releases/0.2.5/Receipt.toml"
printf '\nmodified = true\n' >> "$receipt"
if AOS_HOME="$receipt_home" \
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

write_test_capsule "$legacy_state" codex-code aos-mcp \
  ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff \
  /tmp/user/aos-mcp.capsule \
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
  TEST_AOS_VERSION=2026.1.1 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.2.5
then
  echo "legacy repair unexpectedly committed after host plugin failure" >&2
  exit 1
fi
test -f "$legacy_state/granted-codex-code-codex-install"
test -f "$legacy_state/granted-codex-code-aos-cli"
test ! -e "$legacy_home/extensions/oracles/codex/releases/0.2.5"

TEST_STATE="$legacy_state" TEST_LOG="$legacy_log" \
  TEST_AOS_VERSION=2026.1.1 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.2.5
test ! -e "$legacy_state/granted-codex-code-codex-install"
test ! -e "$legacy_state/granted-codex-code-aos-cli"
test -f "$legacy_state/granted-codex-code-codex-runner"
test -f "$legacy_state/granted-codex-code-aos-fs"
test -f "$legacy_state/granted-codex-code-user-capsule"
test -f "$legacy_state/granted-codex-code-aos-mcp"
test "$(sed -n '1p' "$legacy_state/installed-codex-code-aos-mcp")" \
  = ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
test -f "$legacy_state/installed-codex-code-codex-install"
test -f "$legacy_home/extensions/oracles/codex/releases/0.2.5/ManagedCapsules.toml"
grep -Fq 'name = "aos-mcp"' \
  "$legacy_home/extensions/oracles/codex/releases/0.2.5/ManagedCapsules.toml"
if grep -Eq 'codex-(install|runner)|aos-(cli|fs)' \
  "$legacy_home/extensions/oracles/codex/releases/0.2.5/ManagedCapsules.toml"
then
  echo "new Oracle receipt claimed an obsolete or CE capsule" >&2
  exit 1
fi

# The immutable current pack receipt remains stable when the user keeps a
# same-ID superseding implementation.
receipt_before=$(shasum -a 256 \
  "$legacy_home/extensions/oracles/codex/releases/0.2.5/ManagedCapsules.toml" \
  | awk '{print $1}')
TEST_STATE="$legacy_state" TEST_LOG="$legacy_log" \
  TEST_AOS_VERSION=2026.1.1 AOS_HOME="$legacy_home" \
  AOS_ORACLE_ASSETS="$upgrade_assets" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos \
    --oracle-version 0.2.5
test "$receipt_before" = "$(shasum -a 256 \
  "$legacy_home/extensions/oracles/codex/releases/0.2.5/ManagedCapsules.toml" \
  | awk '{print $1}')"
test "$(sed -n '1p' "$legacy_state/installed-codex-code-aos-mcp")" \
  = ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff

# A live per-home lock fails closed and an unsuccessful contender never removes
# the active installer's lock.
locked_home="$home/locked/.aos"
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
if AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "installer lock without a published diagnostic pid was ignored" >&2
  : > "$lock_release"
  wait "$live_lock_pid"
  exit 1
fi
test ! -s "$lock_path"
printf '%s\n' "$live_lock_pid" > "$lock_path"
if AOS_HOME="$locked_home" \
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
AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
test ! -e "$locked_home/extensions/oracles/.install.lock"

# Missing and malformed stale lock files are reclaimed by the platform lock.
for abandoned in missing malformed; do
  abandoned_home="$home/abandoned-$abandoned/.aos"
  mkdir -p "$abandoned_home/extensions/oracles"
  : > "$abandoned_home/extensions/oracles/.install.lock"
  if [ "$abandoned" = malformed ]; then
    printf '%s\n' not-a-pid > "$abandoned_home/extensions/oracles/.install.lock"
  fi
  AOS_HOME="$abandoned_home" \
    "$repo_root/install.sh" --host codex --yes --no-install-aos
  test ! -e "$abandoned_home/extensions/oracles/.install.lock"
  test -f "$abandoned_home/extensions/oracles/codex/Pack.lock"
done

python3 "$repo_root/scripts/test_release_contract.py"
