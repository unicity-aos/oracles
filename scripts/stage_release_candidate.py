#!/usr/bin/env python3
"""Stage a numbered Oracle RC in a disposable release checkout."""

from pathlib import Path
import re
import sys
import tomllib


def stage(root: Path, version: str) -> None:
    if re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-rc\.[1-9][0-9]*", version) is None:
        raise ValueError("candidate must be a canonical numbered RC")
    base = version.split("-", 1)[0]
    marker = root / "release/oracle-version"
    if marker.read_text().strip() != base:
        raise ValueError("candidate numeric base does not match release source")
    paths = [root / f"packs/{host}.toml" for host in ("claude", "codex", "grok")]
    for path in paths:
        if tomllib.loads(path.read_text())["pack"]["version"] != base:
            raise ValueError(f"pack numeric base does not match: {path}")
    for path in paths:
        path.write_text(path.read_text().replace(f'version = "{base}"', f'version = "{version}"', 1))
    marker.write_text(version + "\n")
    for host in ("claude", "grok", "unicity-aos"):
        (root / f"plugins/{host}/.aos-oracle-version").write_text(version + "\n")


if __name__ == "__main__":
    stage(Path.cwd(), sys.argv[1])
