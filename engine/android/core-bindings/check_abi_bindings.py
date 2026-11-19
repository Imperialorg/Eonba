"""Regenerate both language bindings from the *packaged* target libraries and compare with host."""

import argparse
from pathlib import Path
import subprocess
from zipfile import ZipFile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("bindgen", "host", "apk", "out"):
        parser.add_argument(arg, type=Path)
    args = parser.parse_args()

    def generate(label, library):
        destination = args.out / label
        destination.mkdir(parents=True, exist_ok=True)
        files = ("uniffi/eon_uniffi/eon_uniffi.kt", "eon_uniffi.swift", "eon_uniffiFFI.modulemap", "eon_uniffiFFI.h")
        for name in files:
            (destination / name).unlink(missing_ok=True)
        subprocess.run([args.bindgen, "generate", "--library", "--metadata-no-deps", library,
                        "--language", "kotlin", "--language", "swift", "--no-format",
                        "--out-dir", destination], check=True)
        return {name: (destination / name).read_bytes() for name in files}

    expected = generate("host", args.host)
    with ZipFile(args.apk) as apk:
        for abi in ("arm64-v8a", "armeabi-v7a", "x86_64"):
            library = args.out / abi / "libeon_uniffi.so"
            library.parent.mkdir(parents=True, exist_ok=True)
            library.write_bytes(apk.read(f"lib/{abi}/{library.name}"))
            try:
                actual = generate(abi, library)
                for name in expected:
                    if actual[name] != expected[name]:
                        raise ValueError(f"{abi}: {name} differs from tested host bindings")
            finally:
                library.unlink()
            print(f"{abi}: Kotlin, Swift, C header and module map match host")


if __name__ == "__main__":
    main()
