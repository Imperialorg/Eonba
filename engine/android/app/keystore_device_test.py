"""Build separate Keystore instrumentation; optionally run on an explicitly selected clean device."""

import argparse
import os
from pathlib import Path
import subprocess
from zipfile import ZIP_STORED, ZipFile


APP = Path(__file__).resolve().parent
BUILD = APP / "build"
WORK = BUILD / "intermediates/keystore-test"
SOURCE = APP / "src/androidTest"
TARGET = "com.eon.assistant"
TEST = f"{TARGET}.keystoretest"


def build(sdk):
    tools = sdk / "build-tools/37.0.0"
    platform = sdk / "platforms/android-35/android.jar"
    target_jar = BUILD / "libs/app.jar"
    key = BUILD / "debug.keystore"
    if not target_jar.is_file() or not key.is_file():
        raise SystemExit("Build :app:assembleDebug first.")
    classes = WORK / "classes"
    classes.mkdir(parents=True, exist_ok=True)
    for old in classes.rglob("*.class"):
        old.unlink()
    javac = ["javac", "--release", "8", "-classpath", os.pathsep.join(map(str, (platform, target_jar)))]
    subprocess.run([*javac, "-d", classes, *SOURCE.glob("java/**/*.java")], check=True)
    dex = WORK / "dex.zip"
    subprocess.run([tools / "d8", "--min-api", "28", "--lib", platform, "--classpath", target_jar,
                    "--output", dex, *classes.rglob("*.class")], check=True)
    raw = WORK / "test.apk"
    subprocess.run([tools / "aapt2", "link", "-I", platform, "--manifest", SOURCE / "AndroidManifest.xml",
                    "-o", raw], check=True)
    with ZipFile(raw, "a", compression=ZIP_STORED) as out, ZipFile(dex) as code:
        out.writestr("classes.dex", code.read("classes.dex"))
    aligned = WORK / "aligned.apk"
    signed = WORK / "keystore-test.apk"
    subprocess.run([tools / "zipalign", "-f", "4", raw, aligned], check=True)
    subprocess.run([tools / "apksigner", "sign", "--ks", key, "--ks-pass", "pass:android",
                    "--v4-signing-enabled", "false", "--out", signed, aligned], check=True)
    subprocess.run([tools / "apksigner", "verify", "--min-sdk-version", "28", signed], check=True)
    print(f"Built signed instrumentation: {signed}", flush=True)
    return signed


def run(sdk, serial, test_apk):
    def adb(*args):
        return subprocess.check_output([sdk / "platform-tools/adb", "-s", serial, *args], text=True, timeout=120)

    installed = adb("shell", "pm", "list", "packages", "-u").splitlines()
    if any(f"package:{package}" in installed for package in (TARGET, TEST)):
        raise SystemExit("Refusing to overwrite an existing EON installation. Use a disposable clean device.")
    packages = []
    try:
        for package, apk in ((TARGET, BUILD / "outputs/apk/debug/app-debug.apk"), (TEST, test_apk)):
            if "Success" not in adb("install", "-t", apk).splitlines():
                raise SystemExit("Installation failed.")
            packages.append(package)
        for phase in ("issue", "restart"):
            adb("shell", "am", "force-stop", TARGET)
            lines = adb("shell", "am", "instrument", "-w", "-r", "-e", "phase", phase,
                        f"{TEST}/{TARGET}.KeystoreAcceptance").splitlines()
            if f"INSTRUMENTATION_RESULT: keystore=PASS-{phase}" not in lines or "INSTRUMENTATION_CODE: -1" not in lines:
                status = next((line for line in lines if line.startswith("INSTRUMENTATION_RESULT: keystore=FAIL-")), "")
                raise SystemExit(f"Keystore acceptance failed ({phase}). {status}")
            print(f"PASS: {phase}")
    finally:
        for package in reversed(packages):
            adb("uninstall", package)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--build-only", action="store_true")
    mode.add_argument("--serial", help="adb serial of a clean disposable API 28+ emulator/device")
    args = parser.parse_args()
    sdk = Path(os.environ.get("ANDROID_HOME") or os.environ["ANDROID_SDK_ROOT"])
    apk = build(sdk)
    if args.serial:
        run(sdk, args.serial, apk)
