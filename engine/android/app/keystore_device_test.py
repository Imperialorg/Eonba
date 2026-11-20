"""Build Android instrumentation and run explicitly selected disposable-target acceptance gates."""

import argparse
from contextlib import ExitStack
import hashlib
import os
from pathlib import Path
import re
import shlex
import subprocess
from zipfile import ZIP_STORED, ZipFile


APP = Path(__file__).resolve().parent
BUILD = APP / "build"
WORK = BUILD / "intermediates/keystore-test"
SOURCE = APP / "src/androidTest"
TARGET = "com.eon.assistant"
TEST = f"{TARGET}.keystoretest"
TARGET_APK = BUILD / "outputs/apk/debug/app-debug.apk"


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


def check_result(output, key, expected):
    """am can exit zero on failure. Our raw Instrumentations return one result and one finish code."""
    lines = output.splitlines()
    if lines == [f"INSTRUMENTATION_RESULT: {key}={expected}", "INSTRUMENTATION_CODE: -1"]:
        return
    # Never echo the full device transcript (in particular, credential material or stack traces).
    for line in lines:
        failure = re.fullmatch(rf"INSTRUMENTATION_RESULT: {re.escape(key)}=FAIL-([a-z][a-z0-9-]{{0,47}})", line)
        if failure:
            raise SystemExit(f"{key}={expected} failed at {failure[1]}.")
    raise SystemExit(f"Missing or invalid instrumentation completion for {key}={expected}.")


def require_stock_emulator(adb, serial, minimum_api):
    if not re.fullmatch(r"emulator-[0-9]+", serial):
        raise SystemExit("This gate requires an explicitly selected disposable Android emulator.")
    avd = adb("emu", "avd", "name").splitlines()
    if len(avd) != 2 or not avd[0] or avd[1] != "OK":
        raise SystemExit("Could not verify the selected emulator's AVD.")
    api = adb("shell", "getprop", "ro.build.version.sdk").strip()
    if not re.fullmatch("[0-9]+", api) or int(api) < minimum_api:
        raise SystemExit(f"This gate requires API {minimum_api}+.")


def require_preconfigured_apk(adb):
    # An operator installed and configured the target, not the test APK. Don't replace, clear or
    # uninstall their target. A matching package name (or even debug signer) is not a build ID.
    paths = adb("shell", "pm", "path", TARGET).splitlines()
    if len(paths) != 1 or not paths[0].startswith("package:/"):
        raise SystemExit("Expected the single preconfigured EON debug APK.")
    digest = hashlib.sha256()
    with TARGET_APK.open("rb") as apk:
        for chunk in iter(lambda: apk.read(1024 * 1024), b""):
            digest.update(chunk)
    remote = adb("shell", "sha256sum", shlex.quote(paths[0][len("package:"):])).split()
    if remote[:1] != [digest.hexdigest()]:
        raise SystemExit("Preconfigured target does not match the locally built APK.")


def check_packages(inventory, preconfigured_allowed):
    packages = inventory.splitlines()
    if "package:android" not in packages or not all(line.startswith("package:") for line in packages):
        raise SystemExit("Could not verify installed packages.")
    if f"package:{TEST}" in packages or (not preconfigured_allowed and f"package:{TARGET}" in packages):
        raise SystemExit("Refusing to overwrite an existing EON installation.")
    if preconfigured_allowed and f"package:{TARGET}" not in packages:
        raise SystemExit("Install and configure the target APK before running the allowed gate.")


def run(sdk, serial, test_apk, *, demo_denied=False, stock_hud=False, stock_assistant=False, preconfigured_allowed=False,
        preconfigured_hud=False):
    def adb(*args):
        return subprocess.check_output([sdk / "platform-tools/adb", "-s", serial, *args],
                                       text=True, stderr=subprocess.STDOUT, timeout=120)

    def instrument(component, key, expected, *args):
        output = adb("shell", "am", "instrument", "-w", "-r", *args, f"{TEST}/{TARGET}.{component}")
        check_result(output, key, expected)
        name = expected.removeprefix("PASS-") if key == "keystore" else f"{key}={expected}"
        print(f"PASS: {name}", flush=True)

    preconfigured = preconfigured_allowed or preconfigured_hud
    require_stock_emulator(adb, serial, 34 if demo_denied or stock_hud or stock_assistant or preconfigured else 28)
    check_packages(adb("shell", "pm", "list", "packages", "-u", "--user", "all"), preconfigured)
    with ExitStack() as cleanup:
        def install(package, apk):
            if "Success" not in adb("install", "-t", apk).splitlines():
                raise SystemExit(f"Installation failed for {package}.")
            cleanup.callback(adb, "uninstall", package)

        if preconfigured:
            require_preconfigured_apk(adb)
            install(TEST, test_apk)
            if preconfigured_hud:
                print("Invoke EON with the unlocked assistant gesture. Expand and choose Analyze built-in "
                      "notification tone. After completion, tap Stop and clear; pause, close the card, "
                      "wait a second, then invoke again.",
                      flush=True)
                instrument("OperatorWindowAcceptance", "voiceHud", "PASS-session", "-e", "mode", "session")
            else:
                print("Open a fresh EON action demo. Review and cancel a counter click. Review and confirm "
                      "a click, then a scroll. Choose samples without native actions and repeat the "
                      "confirmed click and scroll. Pause briefly at each result.", flush=True)
                instrument("OperatorWindowAcceptance", "demo", "PASS-allowed", "-e", "mode", "demo")
            return

        install(TARGET, TARGET_APK)
        install(TEST, test_apk)
        for phase in ("issue", "restart"):
            adb("shell", "am", "force-stop", TARGET)
            instrument("KeystoreAcceptance", "keystore", f"PASS-{phase}", "-e", "phase", phase)
        if demo_denied:
            instrument("DemoActionAcceptance", "demo", "PASS-denied", "-e", "mode", "denied")
        if stock_hud:
            instrument("VoiceHudAcceptance", "voiceHud", "PASS-stock", "-e", "mode", "stock")
        if stock_assistant:
            instrument("AssistantActivityAcceptance", "assistantActivity", "PASS-picker-canceled")
        # Only after restart's single-alias and continuity checks; the native vault has its own key.
        instrument("NativeVaultAcceptance", "nativeVault", "PASS")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--build-only", action="store_true")
    mode.add_argument("--serial", help="adb serial of a newly created disposable API 28+ emulator")
    mode.add_argument("--preconfigured-allowed-serial", metavar="SERIAL",
                      help="only run demo=allowed on a disposable unlocked API 34+ AVD: first install "
                           "app-debug.apk and manually select EON as assistant and enable its service and "
                           "shortcut. Leave the instrumentation APK uninstalled. No role or permission grants.")
    mode.add_argument("--preconfigured-hud-serial", metavar="SERIAL",
                      help="observe an operator's real assistant gesture, tone analysis, dismissal and fresh "
                           "invocation on an unlocked disposable API 34+ AVD already configured with EON "
                           "as assistant. Do not enable accessibility. Install only the matching target APK.")
    parser.add_argument("--stock-hud", action="store_true",
                        help="also measure and cancel an actual OS notification tone with the production "
                             "decoder on the unselected clean API 34+ emulator named by --serial")
    parser.add_argument("--demo-denied", action="store_true",
                        help="also require demo=denied on the clean, unlocked API 34+ emulator named by --serial")
    parser.add_argument("--stock-assistant", action="store_true",
                        help="on the clean unlocked API 34+ AVD, launch EON's ACTION_ASSIST activity and "
                             "cancel its real Android role picker without making a selection")
    args = parser.parse_args(argv)
    if args.demo_denied and not args.serial:
        parser.error("--demo-denied requires --serial")
    if args.stock_hud and not args.serial:
        parser.error("--stock-hud requires --serial")
    if args.stock_assistant and not args.serial:
        parser.error("--stock-assistant requires --serial")
    return args


if __name__ == "__main__":
    args = parse_args()
    sdk = Path(os.environ.get("ANDROID_HOME") or os.environ["ANDROID_SDK_ROOT"])
    apk = build(sdk)
    if not args.build_only:
        run(sdk, args.serial or args.preconfigured_allowed_serial or args.preconfigured_hud_serial, apk,
            demo_denied=args.demo_denied, stock_hud=args.stock_hud, stock_assistant=args.stock_assistant,
            preconfigured_allowed=args.preconfigured_allowed_serial is not None,
            preconfigured_hud=args.preconfigured_hud_serial is not None)
