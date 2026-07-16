#!/usr/bin/env sh
# Vendor the shared Unicity AOS plugin scripts from plugins/common/bin into host
# plugins that mirror it, so each host plugin is SELF-CONTAINED.
#
# Why this exists: a plugin marketplace packages ONLY a single plugin's source
# directory (the marketplace manifest declares e.g. `source: ./plugins/claude`).
# A script under plugins/claude/bin therefore cannot reach a sibling
# plugins/common/ at runtime -- that path does not exist in the installed plugin
# cache. A thin wrapper that exec's `../../common/bin/aos-up` dies on launch
# with "No such file or directory", so the MCP stdio server never comes up and
# the host reports `-32000 failed to reconnect`.
#
# plugins/common/bin is the single source of truth. This materializes it into
# each mirroring host's bin/ as a regular file. Run it after editing anything in
# plugins/common/bin; CI verifies the committed copies have not drifted.
#
# Codex is intentionally NOT synced here: it is an independently self-contained
# plugin with its own Codex-specific entrypoint and resolver.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd -P)
COMMON="$ROOT/plugins/common/bin"

# Host plugins whose bin/ mirrors plugins/common/bin verbatim.
MIRROR_HOSTS="claude grok"

# Shared scripts to vendor. Only those a host already ships are copied, so this
# never introduces a script a host deliberately omits (e.g. codex has no
# aos-install, and the mirror hosts each ship their own status lines).
SHARED="aos-up aos-doctor aos-install lib-aos-resolve.sh"

for host in $MIRROR_HOSTS; do
  hb="$ROOT/plugins/$host/bin"
  [ -d "$hb" ] || continue
  for f in $SHARED; do
    [ -f "$COMMON/$f" ] || continue
    [ -e "$hb/$f" ] || continue
    # Drop any existing wrapper OR symlink-into-common first, then copy the real
    # script in as a regular file. A symlink into ../../common only survives
    # today because plugin packaging happens to dereference it -- the same
    # fragile ../../common dependency this script exists to remove.
    rm -f "$hb/$f"
    cp "$COMMON/$f" "$hb/$f"
    chmod 755 "$hb/$f"
  done
done
