"""Check signed executable inventories without requiring a global BLAKE3 tool."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def sha256(path: Path) -> str:
    with path.open('rb') as source:
        digest = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
        return digest.hexdigest()


def verify(executable: Path, expected_blake3: str, expected_sha256: str,
           product: Path, manifest: Path) -> None:
    for digest in (expected_blake3, expected_sha256):
        if re.fullmatch(r'[0-9a-f]{64}', digest) is None:
            raise ValueError('missing signed executable digests')
    # Authenticate bytes independently before using the installed product as
    # a checksum implementation. Neither a runtime nor a damaged product may
    # certify itself by printing the digest we expect.
    if sha256(executable) != expected_sha256:
        raise ValueError('runtime digest does not match its signed record (SHA-256)')
    b3sum = shutil.which('b3sum')
    if b3sum:
        command = [b3sum, str(executable)]
    else:
        home = manifest.parent.parent.parent
        if (not home.is_absolute() or '..' in home.parts
                or product != home / 'bin/aos'
                or manifest.name != 'release-manifest.json'
                or manifest.parent.parent != home / 'releases'):
            raise ValueError('checksum product is not in the canonical release')
        for path in (product, manifest):
            if path.is_symlink() or not path.is_file():
                raise ValueError('checksum product inventory is not a regular file')
            for parent in path.parents:
                if parent.is_symlink() or not parent.is_dir():
                    raise ValueError('checksum product has an unsafe ancestor')
        inventory = json.loads(manifest.read_text())
        record = inventory['release_files']['bin/aos']
        if (inventory.get('schema_version') != 2
                or inventory.get('product', {}).get('version') != manifest.parent.name
                or not isinstance(record, dict)
                or set(record) != {'blake3', 'sha256', 'mode'}
                or record['mode'] != 0o755
                or not os.access(product, os.X_OK)
                or any(not isinstance(record[key], str)
                       or re.fullmatch(r'[0-9a-f]{64}', record[key]) is None
                       for key in ('blake3', 'sha256'))
                or sha256(product) != record['sha256']):
            raise ValueError('checksum product does not match its executable inventory')
        command = [str(product), 'checksum', str(executable)]
    result = subprocess.run(command, check=True, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, timeout=30)
    fields = result.stdout.split()
    if (not fields or fields[0] != expected_blake3
            or (not b3sum and len(fields) != 1)):
        raise ValueError('runtime digest does not match its signed record (BLAKE3)')
    if sha256(executable) != expected_sha256:
        raise ValueError('runtime digest changed during verification')


if __name__ == '__main__':
    try:
        executable, blake3, digest, product, manifest = sys.argv[1:]
        verify(Path(executable), blake3, digest, Path(product), Path(manifest))
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        raise SystemExit(f'aos-resolve: {error}') from error
