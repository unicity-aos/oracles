#!/bin/sh
set -eu

usage() {
    echo "usage: $0 build <artifact-dir> | sign <artifact-dir> <certificate-identity> | inventory <artifact-dir> <expected-file>" >&2
    exit 2
}

primary_assets() {
    printf '%s\n' \
        claude-pack.toml \
        codex-pack.toml \
        grok-pack.toml \
        aos-oracle-plugins.tar.gz \
        BLAKE3SUMS.txt \
        runtime-compatibility.toml
}

expected_assets() {
    primary_assets
    primary_assets | sed 's/$/.sigstore.json/'
}

command=${1:-}
case "$command" in
    build)
        [ "$#" -eq 2 ] || usage
        artifact_dir=$2
        mkdir -p "$artifact_dir"
        artifact_dir=$(cd "$artifact_dir" && pwd)

        for host in claude codex grok; do
            install -m 0644 "packs/$host.toml" "$artifact_dir/$host-pack.toml"
        done
        tar --sort=name --mtime='UTC 1970-01-01' \
            --owner=0 --group=0 --numeric-owner \
            -czf "$artifact_dir/aos-oracle-plugins.tar.gz" \
            .agents .claude-plugin .grok-plugin \
            plugins/claude plugins/grok plugins/unicity-aos
        cp release/runtime-compatibility.toml \
            "$artifact_dir/runtime-compatibility.toml"
        (
            cd "$artifact_dir"
            checksum_input=$(mktemp)
            trap 'rm -f "$checksum_input"' EXIT HUP INT TERM
            b3sum ./*-pack.toml aos-oracle-plugins.tar.gz \
                runtime-compatibility.toml > "$checksum_input"
            sed 's#  \./#  #' < "$checksum_input" > BLAKE3SUMS.txt
        )
        ;;
    sign)
        [ "$#" -eq 3 ] || usage
        artifact_dir=$2
        identity=$3
        [ -n "$identity" ] || {
            echo "certificate identity must not be empty" >&2
            exit 1
        }
        (
            cd "$artifact_dir"
            primary_assets | while IFS= read -r asset; do
                [ -f "$asset" ] || {
                    echo "missing release asset: $asset" >&2
                    exit 1
                }
                cosign sign-blob --yes --bundle "$asset.sigstore.json" "$asset"
                cosign verify-blob \
                    --bundle "$asset.sigstore.json" \
                    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
                    --certificate-identity "$identity" \
                    --use-signed-timestamps \
                    "$asset"
            done
        )
        ;;
    inventory)
        [ "$#" -eq 3 ] || usage
        artifact_dir=$2
        expected_file=$3
        expected_assets | LC_ALL=C sort > "$expected_file"
        while IFS= read -r asset; do
            if [ ! -f "$artifact_dir/$asset" ] || [ -L "$artifact_dir/$asset" ]; then
                echo "release asset must be a regular non-symlink file: $asset" >&2
                exit 1
            fi
        done < "$expected_file"
        actual_file=$(mktemp)
        trap 'rm -f "$actual_file"' EXIT HUP INT TERM
        find "$artifact_dir" -mindepth 1 -maxdepth 1 -exec basename {} \; \
            | LC_ALL=C sort > "$actual_file"
        diff -u "$expected_file" "$actual_file"
        ;;
    *)
        usage
        ;;
esac
