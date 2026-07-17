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
  aos-openai-compat.capsule > "$product_assets/capsule-assets.txt"
for capsule in aos-cli aos-fs aos-openai-compat; do
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
    capsule=${3}
    principal=${*: -1}
    test -f "$TEST_STATE/capsule-$principal-$capsule"
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
    : > "$TEST_STATE/capsule-$principal-$capsule"
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

# The public one-command path installs only marketplace plugins. Host startup
# owns principal and capsule provisioning, so this path must not initialize or
# start AOS and must not create a pack receipt.
plugin_only_home="$home/plugins-only/.aos"
plugin_only_start=$(wc -l < "$TEST_LOG")
AOS_HOME="$plugin_only_home" \
  "$repo_root/install.sh" --plugins-only --host codex --yes --no-install-aos
tail -n "+$((plugin_only_start + 1))" "$TEST_LOG" > "$work/plugin-only.log"
grep -Fq "codex plugin marketplace add $plugin_only_home/extensions/oracles/plugins/0.2.0" \
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
test -d "$prefixed_home/extensions/oracles/plugins/0.2.0"

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
grep -Fq -- '--add-capsule aos-mcp' "$TEST_LOG"
if grep -Eq '^aos --principal codex-code capsule install .*/aos-(cli|fs|openai-compat)\.capsule' "$TEST_LOG"; then
  echo "oracle host provisioning installed a CE distribution capsule" >&2
  exit 1
fi
grep -Fq "codex plugin marketplace add $AOS_HOME/extensions/oracles/plugins/0.2.0" "$TEST_LOG"
grep -Fq 'codex plugin add unicity-aos@unicity-aos-oracles' "$TEST_LOG"
test -L "$AOS_HOME/extensions/oracles/codex/current"
test -f "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
grep -Fq 'source = "local"' "$AOS_HOME/extensions/oracles/codex/current/Receipt.toml"
test ! -e "$AOS_HOME/extensions/oracles/.install.lock"

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

create=$(grep -n 'agent create codex-code' "$TEST_LOG" | head -n1 | cut -d: -f1)
first_install=$(grep -n 'aos --principal codex-code capsule install' "$TEST_LOG" | head -n1 | cut -d: -f1)
test "$create" -lt "$first_install"

# Grok is a separate pack. Installing it provisions only grok-code and aos-mcp,
# installs its own host plugin, writes its own receipt, and leaves any legacy
# Astrid plugin state untouched.
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
cp "$repo_root/packs/claude.toml" "$assets/claude.toml"
claude_start=$(wc -l < "$TEST_LOG")
env -u ANTHROPIC_API_KEY \
  "$repo_root/install.sh" --host claude --yes --no-install-aos
test -f "$AOS_HOME/extensions/oracles/claude/Pack.lock"
tail -n "+$((claude_start + 1))" "$TEST_LOG" > "$work/claude-only.log"
grep -Eq 'capsule install .*/aos-mcp\.capsule$' "$work/claude-only.log"
grep -Fq -- 'agent modify claude-code --add-capsule aos-mcp' "$work/claude-only.log"
grep -Fq 'claude plugin install unicity-aos@unicity-aos-oracles' "$TEST_LOG"
grep -Fq "claude plugin marketplace add $AOS_HOME/extensions/oracles/plugins/0.2.0" "$TEST_LOG"
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
snapshot_manifest="$immutable_home/extensions/oracles/plugins/0.2.0/.agents/plugins/marketplace.json"
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
receipt="$receipt_home/extensions/oracles/codex/releases/0.2.0/Receipt.toml"
printf '\nmodified = true\n' >> "$receipt"
if AOS_HOME="$receipt_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "modified immutable receipt was replaced" >&2
  exit 1
fi
grep -Fq 'modified = true' "$receipt"

# A live per-home lock fails closed and an unsuccessful contender never removes
# the active installer's lock.
locked_home="$home/locked/.aos"
mkdir -p "$locked_home/extensions/oracles/.install.lock"
sleep 60 &
live_lock_pid=$!
printf '%s\n' "$live_lock_pid" > "$locked_home/extensions/oracles/.install.lock/pid"
if AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
then
  echo "concurrent installer lock was ignored" >&2
  kill "$live_lock_pid" 2>/dev/null || true
  exit 1
fi
test "$(cat "$locked_home/extensions/oracles/.install.lock/pid")" = "$live_lock_pid"
kill "$live_lock_pid" 2>/dev/null || true
wait "$live_lock_pid" 2>/dev/null || true

# A lock whose validated owner no longer exists is reclaimed atomically.
printf '%s\n' 999999999 > "$locked_home/extensions/oracles/.install.lock/pid"
AOS_HOME="$locked_home" \
  "$repo_root/install.sh" --host codex --yes --no-install-aos
test ! -e "$locked_home/extensions/oracles/.install.lock"

python3 "$repo_root/scripts/test_release_contract.py"
