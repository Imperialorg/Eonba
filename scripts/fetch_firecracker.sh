#!/usr/bin/env bash
# Fetches the Firecracker microVM test artifacts into a re-creatable cache and verifies each file's SHA-256.
# The binary is checked against the release's published checksum. The guest kernel and initramfs are checked against the
# digests pinned below, which were recorded from the CI bucket at firecracker-ci/v1.13 when this script was written.
#
# Usage: scripts/fetch_firecracker.sh [cache_dir]   (default /tmp/eon-firecracker)
# Then:  EON_FC_BIN=... EON_FC_KERNEL=... EON_FC_INITRD=... cargo test -p eon-capability --test microvm_e2e -- --ignored
set -euo pipefail

FC_VERSION="v1.18.0"
RELEASE_URL="https://github.com/firecracker-microvm/firecracker/releases/download/${FC_VERSION}"
CI_URL="https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.13/x86_64"
KERNEL_SHA256="0e524afe5a90f4ce4b13091a8b5acf7fbe9e7584e711f609d27b69b78564117d"
INITRD_SHA256="1ab40cb09aae10cf47d84754f10e9d4710de62cd07d7d54e412dfddd7586f4d0"

cache="${1:-/tmp/eon-firecracker}"
mkdir -p "$cache"
cd "$cache"

fetch_verified() {
    local url="$1" out="$2" expected="$3"
    if [[ -f "$out" ]] && [[ "$(sha256sum "$out" | cut -d' ' -f1)" == "$expected" ]]; then
        return 0
    fi
    curl -sSfL --max-time 600 -o "$out" "$url"
    local got
    got="$(sha256sum "$out" | cut -d' ' -f1)"
    if [[ "$got" != "$expected" ]]; then
        echo "checksum mismatch for $out: expected $expected, got $got" >&2
        rm -f "$out"
        exit 1
    fi
}

curl -sSfL --max-time 60 -o fc.tgz.sha256 "${RELEASE_URL}/firecracker-${FC_VERSION}-x86_64.tgz.sha256.txt"
curl -sSfL --max-time 600 -o fc.tgz "${RELEASE_URL}/firecracker-${FC_VERSION}-x86_64.tgz"
if [[ "$(sha256sum fc.tgz | cut -d' ' -f1)" != "$(cut -d' ' -f1 fc.tgz.sha256)" ]]; then
    echo "firecracker release checksum mismatch" >&2
    exit 1
fi
tar xzf fc.tgz
cp "release-${FC_VERSION}-x86_64/firecracker-${FC_VERSION}-x86_64" ./firecracker
chmod +x ./firecracker

fetch_verified "${CI_URL}/vmlinux-5.10.239" vmlinux "$KERNEL_SHA256"
fetch_verified "${CI_URL}/initramfs.cpio" initramfs.cpio "$INITRD_SHA256"

echo "EON_FC_BIN=${cache}/firecracker"
echo "EON_FC_KERNEL=${cache}/vmlinux"
echo "EON_FC_INITRD=${cache}/initramfs.cpio"
