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
for capsule in \
  aos-mcp claude-install claude-runner codex-install codex-runner
do
  printf 'signed fixture for %s\n' "$capsule" > "$assets/$capsule.capsule"
done

write_fixture_checksums() {
  root=$1
  : > "$root/BLAKE3SUMS.txt"
  for asset in \
    aos-mcp.capsule \
    claude-install.capsule claude-pack.toml claude-runner.capsule \
    codex-install.capsule codex-pack.toml codex-runner.capsule \
    grok-pack.toml aos-oracle-plugins.tar.gz runtime-compatibility.toml
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
    test -f "$AOS_HOME/runtime-running"
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
  *" capsule show aos-cli --agent "*)
    principal=${*: -1}
    test -f "$TEST_STATE/product-$principal"
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
test "$(grep -Fc 'aos --principal default init --yes --offline' "$TEST_LOG")" -eq 1
grep -Fq 'aos --principal default init --target-principal codex-code --yes --offline' "$TEST_LOG"
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
for capsule in aos-mcp codex-install codex-runner; do
  grep -Eq "capsule install .*/$capsule\\.capsule" "$TEST_LOG"
  grep -Fq -- "--add-capsule $capsule" "$TEST_LOG"
done
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
  aos-oracle-plugins.tar.gz runtime-compatibility.toml codex.toml \
  aos-mcp.capsule codex-install.capsule codex-runner.capsule
do
  cp "$assets/$asset" "$minimal_assets/$asset"
done
: > "$minimal_assets/BLAKE3SUMS.txt"
for asset in \
  aos-mcp.capsule codex-pack.toml codex-install.capsule codex-runner.capsule \
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
init_count=$(grep -Fc 'aos --principal default init --yes --offline' "$TEST_LOG")
target_init_count=$(grep -Fc 'aos --principal default init --target-principal codex-code --yes --offline' "$TEST_LOG")
start_count=$(grep -Fc 'aos --principal default start' "$TEST_LOG")
"$repo_root/install.sh" --host codex --yes --no-install-aos
test "$first_lock" = "$(shasum -a 256 "$lock" | awk '{print $1}')"
test "$(grep -Fc 'aos --principal default init --yes --offline' "$TEST_LOG")" -eq "$init_count"
test "$(grep -Fc 'aos --principal default init --target-principal codex-code --yes --offline' "$TEST_LOG")" -eq "$target_init_count"
test "$(grep -Fc 'aos --principal default start' "$TEST_LOG")" -eq "$start_count"
if grep -Fq 'aos --principal default stop' "$TEST_LOG"; then
  echo "repeat oracle install stopped the shared runtime" >&2
  exit 1
fi

create=$(grep -n 'agent create codex-code' "$TEST_LOG" | head -n1 | cut -d: -f1)
first_install=$(grep -n 'capsule install' "$TEST_LOG" | head -n1 | cut -d: -f1)
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

# A fresh non-interactive Claude install must fail before grants, receipt, or
# plugin installation when its selected API-key mode has no credential.
cp "$repo_root/packs/claude.toml" "$assets/claude.toml"
for capsule in claude-install claude-runner; do
  printf 'signed fixture for %s\n' "$capsule" > "$assets/$capsule.capsule"
done
if env -u ANTHROPIC_API_KEY \
  "$repo_root/install.sh" --host claude --yes --no-install-aos \
    --claude-auth subscription --claude-mode headless
then
  echo "Claude headless subscription mode unexpectedly succeeded" >&2
  exit 1
fi
claude_start=$(wc -l < "$TEST_LOG")
if env -u ANTHROPIC_API_KEY \
  "$repo_root/install.sh" --host claude --yes --no-install-aos
then
  echo "Claude --yes unexpectedly succeeded without ANTHROPIC_API_KEY" >&2
  exit 1
fi
test ! -e "$AOS_HOME/extensions/oracles/claude/Pack.lock"
tail -n "+$((claude_start + 1))" "$TEST_LOG" > "$work/claude-failed.log"
if grep -Fq 'agent modify claude-code' "$work/claude-failed.log"; then
  echo "failed Claude install modified its principal" >&2
  exit 1
fi
if grep -Fq 'claude plugin' "$work/claude-failed.log"; then
  echo "failed Claude install changed its plugin" >&2
  exit 1
fi

# With the explicit secret present, the CLI's headless lifecycle responder is
# selected and the pack converges normally without stdin.
export ANTHROPIC_API_KEY=not-a-real-anthropic-key
"$repo_root/install.sh" --host claude --yes --no-install-aos
test -f "$AOS_HOME/extensions/oracles/claude/Pack.lock"
grep -Eq 'capsule install .*/claude-runner\.capsule$' "$TEST_LOG"
grep -Fq 'agent modify claude-code' "$TEST_LOG"
grep -Fq 'claude plugin install unicity-aos@unicity-aos-oracles' "$TEST_LOG"
grep -Fq "claude plugin marketplace add $AOS_HOME/extensions/oracles/plugins/0.2.0" "$TEST_LOG"

# Subscription auth is an interactive Claude Code REPL mode and does not
# require an API key.
subscription_home="$home/subscription/.aos"
env -u ANTHROPIC_API_KEY AOS_HOME="$subscription_home" \
  "$repo_root/install.sh" --host claude --yes --no-install-aos \
    --claude-auth subscription --claude-mode repl
test -f "$subscription_home/extensions/oracles/claude/Pack.lock"
grep -Fq 'claude-vars auth=subscription interaction=repl api-key-set=no' "$TEST_LOG"

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
printf 'tampered\n' >> "$tampered_assets/codex-runner.capsule"
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
