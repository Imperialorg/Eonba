package com.eon.assistant;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Activity;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityNodeInfo;
import java.util.List;
import java.util.function.BooleanSupplier;

/** Separate test process. Observe only visible EON controls; never grant permissions or invoke a session. */
public final class SessionMicAcceptance extends Instrumentation {
    private UiAutomation ui;
    private String mode;
    private String step = "preflight";

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        mode = args.getString("mode");
        start();
    }

    private String text(String id) {
        AccessibilityNodeInfo root = ui.getRootInActiveWindow();
        if (root == null || !"com.eon.assistant".contentEquals(root.getPackageName())) return "";
        List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId("com.eon.assistant:id/" + id);
        if (nodes.size() != 1 || !nodes.get(0).isVisibleToUser() || nodes.get(0).getText() == null) return "";
        return nodes.get(0).getText().toString();
    }

    private void await(String name, BooleanSupplier predicate) {
        step = name;
        long end = SystemClock.elapsedRealtime() + 40_000;
        do {
            if (predicate.getAsBoolean()) return;
            SystemClock.sleep(70);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        int code = Activity.RESULT_CANCELED;
        try {
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
            AccessibilityServiceInfo info = ui.getServiceInfo();
            info.packageNames = new String[]{"com.eon.assistant"};
            info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            ui.setServiceInfo(info);
            if ("denied".equals(mode)) {
                await("denied", () -> text("hud_mic_status").startsWith("Mic off. Use Open EON controls"));
            } else {
                await("progress", () -> text("hud_mic_status").matches(
                        "Measuring microphone · (?:[1-9]|1[01])/12 half-second levels · RMS (?:[0-9]{1,2}|100)%") &&
                        text("hud_mic").equalsIgnoreCase("Stop microphone now"));
                if ("complete".equals(mode)) {
                    await("complete", () -> text("hud_mic_status").matches(
                            "Mic off · 12 levels complete · highest RMS (?:[0-9]{1,2}|100)%"));
                    result.putString("completion", text("hud_mic_status"));
                } else if ("stop".equals(mode)) {
                    await("stop", () -> "Mic off · stopped · levels cleared".equals(text("hud_mic_status")));
                } else if ("hide".equals(mode)) {
                    await("hide", () -> text("hud_title").isEmpty());
                    await("fresh", () -> "Session open".equals(text("hud_status")) &&
                            "EON · Mic off".equals(text("hud_title")) && text("hud_mic_status").isEmpty());
                } else throw new AssertionError();
            }
            result.putString("micHud", "PASS-" + mode);
            code = Activity.RESULT_OK;
        } catch (RuntimeException | AssertionError | LinkageError failure) {
            result.putString("micHud", "FAIL-" + step);
        }
        finish(code, result);
    }
}
