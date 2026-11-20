#!/usr/bin/env bash
# Offline provisioning, NOT a workspace tool. Supply already vetted Node
# 24.22.1, the extracted official pyright@1.1.414 package, postject
# 1.0.0-alpha.6's dist/cli.js, and a NEW installation in a protected parent.
# No npm lifecycle scripts, application build, or download is performed here.
set -euo pipefail
umask 077
[[ $# == 4 ]] || { echo "usage: $0 NODE PYRIGHT_PACKAGE POSTJECT_CLI NEW_INSTALL" >&2; exit 2; }
node=$(realpath -e -- "$1")
package=$(realpath -e -- "$2")
postject=$(realpath -e -- "$3")
install=$(realpath -m -- "$4")
here=$(dirname -- "$(realpath -e -- "$0")")
[[ $("$node" --version) == v24.22.1 ]]
mkdir -m 700 -- "$install"
build="$install/build"
mkdir -m 700 -- "$build"
"$node" "$here/pyright-sea.cjs" "$package/dist" "$build"
"$node" --experimental-sea-config "$build/sea.json"
image="$install/pyright-langserver"
cp -- "$node" "$image"
"$node" "$postject" "$image" NODE_SEA_BLOB "$build/sea.blob" \
    --sentinel-fuse NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2
chmod 500 "$image"
mkdir -m 700 "$install/typeshed-fallback"
cp -R -- "$package/dist/typeshed-fallback/stdlib" "$package/dist/typeshed-fallback/LICENSE" "$install/typeshed-fallback/"
cp -- "$package/LICENSE.txt" "$install/Pyright-LICENSE.txt"
rm -- "$build/main.cjs" "$build/sea.json" "$build/sea.blob"
rmdir -- "$build"
# Record at provisioning time in independently protected host configuration;
# never approve an image by recalculating the expected digest at launch.
sha256sum -- "$image"
