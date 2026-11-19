#!/usr/bin/env bash
# Regenerates the Kotlin UniFFI bindings from the built eon-uniffi library, compiles them with the smoke test, and runs
# it against the native library. Build outputs go to /tmp.
#
# Requires: JAVA_HOME (JDK 17+), KOTLINC (path to the kotlinc binary), JNA_JAR, KOTLIN_STDLIB_JAR.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core="$here/../../.."
out="${EON_KOTLIN_OUT:-/tmp/eon-uniffi-kotlin}"
target="${CARGO_TARGET_DIR:-/tmp/cargo-target}"
: "${JAVA_HOME:?set JAVA_HOME to a JDK 17+}"
: "${KOTLINC:?set KOTLINC to the kotlinc binary}"
: "${JNA_JAR:?set JNA_JAR to jna.jar}"
: "${KOTLIN_STDLIB_JAR:?set KOTLIN_STDLIB_JAR to kotlin-stdlib.jar}"

rm -rf "$out" && mkdir -p "$out"
(cd "$core" && cargo build -q -p eon-uniffi)
(cd "$core" && cargo run -q -p eon-uniffi --bin uniffi-bindgen -- generate \
    --library "$target/debug/libeon_uniffi.so" --language kotlin --out-dir "$out/bindings" --no-format)
"$KOTLINC" "$out/bindings/uniffi/eon_uniffi/eon_uniffi.kt" "$here/Smoke.kt" -cp "$JNA_JAR" -d "$out/classes"
"$JAVA_HOME/bin/java" -Djna.library.path="$target/debug" \
    -cp "$out/classes:$JNA_JAR:$KOTLIN_STDLIB_JAR" SmokeKt
