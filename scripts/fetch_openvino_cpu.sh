#!/usr/bin/env bash
# x86-64 Linux, opt in only. Extract the native ONNX frontend and CPU runtime;
# no Python install, GPU plugins, model downloads, or telemetry package.
# Add the printed absolute directory to LD_LIBRARY_PATH alongside ORT 1.31.0.
# Select EON_ICON_BACKEND=openvino-cpu and Cargo feature omniparser-openvino.
set -euo pipefail
umask 077
[[ "$(uname -s):$(uname -m)" == "Linux:x86_64" ]] || { echo 'Requires x86-64 Linux' >&2; exit 1; }
repo="$(cd "$(dirname "$0")/.." && pwd)"
parent="$repo/engine/core-rs/target"
dir="$parent/openvino-cpu-2026.4.1"
mkdir -p "$parent"
# Do not overwrite any existing native libraries or follow destination symlinks.
mkdir "$dir" || { echo "Expected a new directory: $dir" >&2; exit 1; }
cd "$dir"
name=openvino-2026.4.1-22982-cp312-cp312-manylinux_2_28_x86_64.whl
url="https://files.pythonhosted.org/packages/ec/3a/2a173ac1ad749ff0b041788eefc1ade0d410231fedfc43f77474f3b806cc/$name"
curl --proto '=https' --fail --silent --show-error --max-time 180 --max-filesize 59144434 "$url" -o "$name"
echo "6b4375c17ddcac83a5180349e2e2bb811185c261066e2a920659892d58ef0e3b  $name" | sha256sum --check --status
unzip -jq "$name" openvino/libs/{libopenvino_c.so.2641,libopenvino.so.2641,libopenvino_onnx_frontend.so.2641,libopenvino_intel_cpu_plugin.so,libtbb.so.12,libtbbbind_2_5.so.3,libhwloc.so.15,libtbbmalloc.so.2}
ln -s libopenvino_c.so.2641 libopenvino_c.so
printf '%s\n' "$dir"
