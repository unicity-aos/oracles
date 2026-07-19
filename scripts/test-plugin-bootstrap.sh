#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
home="$work/home"
log="$work/install.log"
mkdir -p "$home"

fake_installer="$work/fake-oracle-installer.sh"
cat > "$fake_installer" <<'EOF'
#!/usr/bin/env sh
set -eu
printf '%s\n' "$*" >> "$TEST_INSTALL_LOG"
host=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --host) shift; host=${1:-} ;;
  esac
  shift
done
[ "$host" = codex ] || exit 91
mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/codex"
printf '%s\n' 'version = "0.2.5"' > "$AOS_HOME/extensions/oracles/codex/Pack.lock"
cat > "$AOS_HOME/bin/aos" <<'AOS'
#!/usr/bin/env sh
set -eu
case " ${*:-} " in
  *" --version "*) printf 'Unicity AOS 2026.1.0\n' ;;
  *" capsule show aos-mcp "*) exit 0 ;;
  *" status --json "*)
    [ "${TEST_STOPPED:-0}" -eq 0 ] || exit 1
    printf '%s\n' '{"state":"running","pid":4242,"loaded_capsules":["aos-mcp"]}'
    ;;
  *" update --check "*)
    [ "${TEST_UPDATE_AVAILABLE:-0}" -eq 1 ] \
      && printf '%s\n' 'Update available: Unicity AOS 2026.1.0 -> 2026.1.1. Run `aos update` to install.'
    exit 0
    ;;
  *" emit --topic "*)
    pwd -P > "$TEST_HOOK_AOS_CWD"
    cat > "$TEST_HOOK_PAYLOAD"
    ;;
  *) exit 0 ;;
esac
AOS
chmod 700 "$AOS_HOME/bin/aos"
EOF
chmod 700 "$fake_installer"

output=$(env -i \
  PATH=/usr/bin:/bin \
  HOME="$home" \
  AOS_HOME="$home/.aos" \
  AOS_PLUGIN_ROOT="$repo_root/plugins/unicity-aos" \
  AOS_ORACLES_INSTALLER="$fake_installer" \
  TEST_INSTALL_LOG="$log" \
  "$repo_root/plugins/unicity-aos/bin/aos-doctor" --format hook </dev/null)

python3 - "$output" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
context = payload["hookSpecificOutput"]["additionalContext"]
assert "Unicity AOS is ready for this Codex session" in context
assert "codex-code" in context
assert "Capsules may contribute durable, principal-scoped skills" in context
assert 'dir_path "skills"' in context
assert "read_skill" in context
PY

test -x "$home/.aos/bin/aos"
test -f "$home/.aos/extensions/oracles/codex/Pack.lock"
test "$(wc -l < "$log" | tr -d ' ')" = 1

rm -rf "$home/.aos/update/host-update-check"
update_output=$(env -i \
  PATH=/usr/bin:/bin \
  HOME="$home" \
  AOS_HOME="$home/.aos" \
  AOS_PLUGIN_ROOT="$repo_root/plugins/unicity-aos" \
  AOS_ORACLES_INSTALLER="$fake_installer" \
  TEST_INSTALL_LOG="$log" \
  TEST_UPDATE_AVAILABLE=1 \
  "$repo_root/plugins/unicity-aos/bin/aos-doctor" --format hook </dev/null)
python3 - "$update_output" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
assert payload["systemMessage"].startswith("Update available: Unicity AOS")
PY
grep -Fq -- '--host codex' "$log"
grep -Fq -- '--skip-host-plugin' "$log"
grep -Fq -- '--yes' "$log"
grep -Fq -- '--oracle-version 0.2.5' "$log"
if grep -Eq -- '--host (claude|grok)' "$log"; then
  echo "Codex bootstrap attempted to install another host" >&2
  exit 1
fi

# With a healthy base and Codex pack, a stopped runtime is ready for the MCP
# adapter to start on demand. Another startup is read-only and never re-enters
# the installer.
stopped_output=$(env -i \
  PATH=/usr/bin:/bin \
  HOME="$home" \
  AOS_HOME="$home/.aos" \
  AOS_PLUGIN_ROOT="$repo_root/plugins/unicity-aos" \
  AOS_ORACLES_INSTALLER="$fake_installer" \
  TEST_INSTALL_LOG="$log" \
  TEST_STOPPED=1 \
  "$repo_root/plugins/unicity-aos/bin/aos-doctor" --format hook </dev/null)
python3 - "$stopped_output" <<'PY'
import json
import sys

context = json.loads(sys.argv[1])["hookSpecificOutput"]["additionalContext"]
assert "Unicity AOS is ready for this Codex session" in context
assert "starts on MCP connect" in context
PY
test "$(wc -l < "$log" | tr -d ' ')" = 1

# Runtime IPC always uses the product workspace, but hook context retains the
# actual Codex project so capsules receive the host's real working directory.
project="$work/user-project"
hook_payload="$work/hook-payload.json"
hook_aos_cwd="$work/hook-aos-cwd"
mkdir -p "$project"
(cd "$project" && printf '%s\n' '{"session_id":"release-smoke"}' | env -i \
  PATH=/usr/bin:/bin \
  HOME="$home" \
  AOS_HOME="$home/.aos" \
  AOS_PLUGIN_ROOT="$repo_root/plugins/unicity-aos" \
  TEST_INSTALL_LOG="$log" \
  TEST_HOOK_PAYLOAD="$hook_payload" \
  TEST_HOOK_AOS_CWD="$hook_aos_cwd" \
  "$repo_root/plugins/unicity-aos/bin/aos-up" codex hook user_prompt_submit)
python3 - "$hook_payload" "$project" "$hook_aos_cwd" "$home/.aos/runtime" <<'PY'
import base64
import json
from pathlib import Path
import sys

payload = json.loads(Path(sys.argv[1]).read_text())
assert base64.b64decode(payload["cwd_b64"]).decode().strip() == str(Path(sys.argv[2]).resolve())
assert Path(sys.argv[3]).read_text().strip() == str(Path(sys.argv[4]).resolve())
PY

# The startup update nudge checks only the AOS channel, is cached, and never
# invokes a host plugin command.
check_home="$work/check-home"
check_log="$work/check.log"
mkdir -p "$check_home/bin"
cat > "$check_home/bin/aos" <<'EOF'
#!/usr/bin/env sh
set -eu
printf '%s\n' "$*" >> "$TEST_CHECK_LOG"
[ "$*" = 'update --check' ]
printf '%s\n' 'Update available: Unicity AOS 2026.1.0 -> 2026.1.1. Run `aos update` to install.'
EOF
chmod 700 "$check_home/bin/aos"

first=$(AOS_HOME="$check_home" TEST_CHECK_LOG="$check_log" \
  "$repo_root/plugins/common/bin/aos-update-check" "$check_home/bin/aos")
second=$(AOS_HOME="$check_home" TEST_CHECK_LOG="$check_log" \
  "$repo_root/plugins/common/bin/aos-update-check" "$check_home/bin/aos")
test "$first" = "$second"
test "$(wc -l < "$check_log" | tr -d ' ')" = 1
grep -Fxq 'update --check' "$check_log"

failure_home="$work/failure-home"
failure_log="$work/failure.log"
mkdir -p "$failure_home/bin"
cat > "$failure_home/bin/aos" <<'EOF'
#!/usr/bin/env sh
printf '%s\n' "$*" >> "$TEST_CHECK_LOG"
exit 69
EOF
chmod 700 "$failure_home/bin/aos"
failure=$(AOS_HOME="$failure_home" TEST_CHECK_LOG="$failure_log" \
  "$repo_root/plugins/common/bin/aos-update-check" "$failure_home/bin/aos")
test -z "$failure"
grep -Fxq 'update --check' "$failure_log"
