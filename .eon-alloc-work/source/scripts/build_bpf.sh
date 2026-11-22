#!/usr/bin/env bash
# Compiles the eBPF probes in engine/bpf to BPF objects. The output directory is re-creatable (default /tmp/eon-bpf).
# Usage: scripts/build_bpf.sh [out_dir]   then: EON_BPF_OBJ=<out_dir>/exec_count.bpf.o
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-/tmp/eon-bpf}"
mkdir -p "$out"
clang -O2 -g -target bpf -c "$here/engine/bpf/exec_count.bpf.c" -o "$out/exec_count.bpf.o"
echo "EON_BPF_OBJ=$out/exec_count.bpf.o"
