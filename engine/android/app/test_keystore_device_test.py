"""Host-side transcript/CLI checks. These tests do not stand in for Android instrumentation."""

import contextlib
import io
import unittest

from keystore_device_test import TARGET, TEST, check_packages, check_result, parse_args


class InstrumentationResults(unittest.TestCase):
    def test_requires_result_for_the_requested_gate_and_successful_finish(self):
        for key, result in (("keystore", "PASS-issue"), ("keystore", "PASS-restart"),
                            ("demo", "PASS-denied"), ("demo", "PASS-allowed"), ("nativeVault", "PASS")):
            with self.subTest(key=key, result=result):
                check_result(f"INSTRUMENTATION_RESULT: {key}={result}\r\nINSTRUMENTATION_CODE: -1\r\n", key, result)

    def test_neither_a_stale_pass_nor_am_exit_zero_is_a_completion(self):
        result = "INSTRUMENTATION_RESULT: demo=PASS-denied\n"
        code = "INSTRUMENTATION_CODE: -1\n"
        for output in ("", result, code, result + "INSTRUMENTATION_CODE: 0\n",
                       result.replace("denied", "allowed") + code, result.replace("demo", "keystore") + code,
                       result + result + code, result + code + "INSTRUMENTATION_CODE: 0",
                       "INSTRUMENTATION_FAILED: Process crashed.\n" + result + code):
            with self.subTest(output=output), self.assertRaises(SystemExit):
                check_result(output, "demo", "PASS-denied")

    def test_reports_failure_checkpoint_not_arbitrary_device_output(self):
        with self.assertRaisesRegex(SystemExit, r"^keystore=PASS-restart failed at key-continuity\.$"):
            check_result("INSTRUMENTATION_RESULT: keystore=FAIL-key-continuity\nprovider details\n"
                         "INSTRUMENTATION_CODE: 0\n", "keystore", "PASS-restart")


class DeviceModes(unittest.TestCase):
    def test_allowed_is_opt_in_and_separate_from_clean_install(self):
        clean = parse_args(["--serial", "emulator-5554", "--demo-denied"])
        self.assertTrue(clean.demo_denied)
        self.assertIsNone(clean.preconfigured_allowed_serial)
        allowed = parse_args(["--preconfigured-allowed-serial", "emulator-5556"])
        self.assertEqual(allowed.preconfigured_allowed_serial, "emulator-5556")
        self.assertIsNone(allowed.serial)
        self.assertFalse(allowed.demo_denied)
        self.assertFalse(parse_args(["--serial", "emulator-5554"]).demo_denied)

    def test_incompatible_requests_are_rejected_not_silently_skipped(self):
        for args in (["--demo-denied"], ["--build-only", "--demo-denied"],
                     ["--preconfigured-allowed-serial", "emulator-5556", "--demo-denied"],
                     ["--preconfigured-allowed-serial", "emulator-5556", "--serial", "emulator-5554"]):
            with self.subTest(args=args), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parse_args(args)

    def test_only_the_separate_allowed_gate_accepts_an_installed_target(self):
        clean = "package:android\n"
        target = clean + f"package:{TARGET}\n"
        existing_test = f"package:{TEST}\n"
        check_packages(clean, False)
        check_packages(target, True)
        for inventory, allowed in ((target, False), (clean, True), (clean + existing_test, False),
                                   (target + existing_test, True), ("Error: package manager unavailable\n", False),
                                   (clean + "Error: cannot list packages for user\n", False)):
            with self.subTest(inventory=inventory, allowed=allowed), self.assertRaises(SystemExit):
                check_packages(inventory, allowed)


if __name__ == "__main__":
    unittest.main()
