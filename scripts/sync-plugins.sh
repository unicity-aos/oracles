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
# Codex keeps its own entrypoint, doctor, and resolver. It shares only the
# idempotent host-pack installer.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd -P)
COMMON="$ROOT/plugins/common/bin"

# Host plugins whose bin/ mirrors plugins/common/bin verbatim.
MIRROR_HOSTS="claude grok"

# Shared scripts to vendor into every mirrored host plugin. Host-specific
# scripts such as status lines are deliberately absent from this list.
SHARED="aos-up aos-doctor aos-install aos-update-check lib-aos-resolve.sh"

for host in $MIRROR_HOSTS; do
  hb="$ROOT/plugins/$host/bin"
  [ -d "$hb" ] || continue
  for f in $SHARED; do
    [ -f "$COMMON/$f" ] || continue
    # Drop any existing wrapper OR symlink-into-common first, then copy the real
    # script in as a regular file. A symlink into ../../common only survives
    # today because plugin packaging happens to dereference it -- the same
    # fragile ../../common dependency this script exists to remove.
    rm -f "$hb/$f"
    cp "$COMMON/$f" "$hb/$f"
    chmod 755 "$hb/$f"
  done
done

for host in claude grok unicity-aos; do
  [ -d "$ROOT/plugins/$host" ] || continue
  cp "$ROOT/release/oracle-version" "$ROOT/plugins/$host/.aos-oracle-version"
  chmod 644 "$ROOT/plugins/$host/.aos-oracle-version"
done

if [ -d "$ROOT/plugins/unicity-aos/bin" ]; then
  for f in aos-install aos-update-check; do
    cp "$COMMON/$f" "$ROOT/plugins/unicity-aos/bin/$f"
    chmod 755 "$ROOT/plugins/unicity-aos/bin/$f"
  done
fi
