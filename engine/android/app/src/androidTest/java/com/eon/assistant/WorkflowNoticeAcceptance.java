package com.eon.assistant;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Activity;
import android.app.ActivityManager;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.text.TextUtils;
import android.view.View;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import java.util.List;
import java.util.function.Predicate;

/** Observe the selected assistant from the test APK; do not start a session, click, or grant access. */
public final class WorkflowNoticeAcceptance extends Instrumentation {
    private static final String EON = "com.eon.assistant";
    private static final String ID = EON + ":id/";
    private UiAutomation ui;
    private int card;
    private volatile boolean checkClicked;
    private String step = "preflight";

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        start();
    }

    private static void require(boolean condition) {
        if (!condition) throw new AssertionError();
    }

    private static AccessibilityNodeInfo visible(AccessibilityNodeInfo root, String id) {
        if (root == null) return null;
        List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId(ID + id);
        if (nodes.isEmpty()) return null;
        require(nodes.size() == 1);
        AccessibilityNodeInfo n = nodes.get(0);
        require(n.getWindowId() == root.getWindowId() && TextUtils.equals(EON, n.getPackageName()));
        return n.isVisibleToUser() ? n : null;
    }

    private static String text(AccessibilityNodeInfo root, String id) {
        AccessibilityNodeInfo n = visible(root, id);
        return n == null || n.getText() == null ? "" : n.getText().toString();
    }

    private AccessibilityNodeInfo await(String checkpoint, Predicate<AccessibilityNodeInfo> expected) {
        step = checkpoint;
        long end = SystemClock.elapsedRealtime() + 30_000;
        while (SystemClock.elapsedRealtime() < end) {
            ui.clearCache();
            AccessibilityNodeInfo root = ui.getRootInActiveWindow();
            if (root != null && TextUtils.equals(EON, root.getPackageName()) && expected.test(root)) return root;
            SystemClock.sleep(80);
        }
        throw new AssertionError();
    }

    private static boolean idle(AccessibilityNodeInfo r, boolean expanded) {
        AccessibilityNodeInfo toggle = visible(r, "hud_toggle");
        return toggle != null && TextUtils.equals(toggle.getContentDescription(),
                        expanded ? "Hide session details" : "Show session details")
                && "Session open".equals(text(r, "hud_status")) && visible(r, "hud_result") == null
                && (expanded ? "Check model profile".equalsIgnoreCase(text(r, "hud_check"))
                             : visible(r, "hud_check") == null);
    }

    private static boolean controls(AccessibilityNodeInfo r) {
        return "EON is the selected assistant.".equals(text(r, "assistant_role_status"))
                && visible(r, "hud_status") == null && visible(r, "hud_result") == null;
    }

    private void observe() {
        // This fixture intentionally has <8 decimal GB, so the profile is known independently
        // of both the generated bindings and the text the application happens to show.
        ActivityManager.MemoryInfo memory = new ActivityManager.MemoryInfo();
        getTargetContext().getSystemService(ActivityManager.class).getMemoryInfo(memory);
        require(memory.totalMem > 0 && memory.totalMem < 8_000_000_000L);
        await("selected-controls", WorkflowNoticeAcceptance::controls);
        card = await("fresh-card", r -> idle(r, false)).getWindowId();
        await("ready-check", r -> r.getWindowId() == card && idle(r, true));

        final long since = SystemClock.uptimeMillis();
        ui.setOnAccessibilityEventListener(event -> {
            if (event.getEventTime() < since || event.getEventType() != AccessibilityEvent.TYPE_VIEW_CLICKED
                    || event.getWindowId() != card || !TextUtils.equals(EON, event.getPackageName())) return;
            AccessibilityNodeInfo n = event.getSource(0);
            if (n != null && n.getWindowId() == card && TextUtils.equals(EON, n.getPackageName())
                    && (ID + "hud_check").equals(n.getViewIdResourceName())) checkClicked = true;
        });
        await("explicit-check", r -> r.getWindowId() == card && checkClicked);
        await("completed-receipt", r -> {
            AccessibilityNodeInfo status = visible(r, "hud_status");
            return r.getWindowId() == card && status != null
                    && status.getLiveRegion() == View.ACCESSIBILITY_LIVE_REGION_POLITE
                    && TextUtils.equals("Profile workflow complete. Completed tasks: 1", status.getText())
                    && "Suggested profile: p0_mobile_ultralight".equals(text(r, "hud_result"));
        });

        // A missing root, a background/foreign window or a collapsed card isn't a dismissal.
        await("dismissed-to-controls", r -> r.getWindowId() != card && controls(r));
        int fresh = await("reshown-card", r -> idle(r, false)).getWindowId();
        await("reshown-details", r -> r.getWindowId() == fresh && idle(r, true));
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        int code = Activity.RESULT_CANCELED;
        try {
            require(Build.VERSION.SDK_INT >= 34 && ("ranchu".equals(Build.HARDWARE) || "goldfish".equals(Build.HARDWARE))
                    && (EON + ".keystoretest").equals(getTargetContext().getPackageName()));
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
            AccessibilityServiceInfo info = ui.getServiceInfo();
            info.packageNames = new String[]{EON};
            info.eventTypes = AccessibilityEvent.TYPE_VIEW_CLICKED;
            info.notificationTimeout = 0;
            info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            ui.setServiceInfo(info);
            observe();
            result.putString("workflowNotice", "PASS-session");
            code = Activity.RESULT_OK;
        } catch (RuntimeException | AssertionError | LinkageError failure) {
            result.putString("workflowNotice", "FAIL-" + step);
        } finally {
            if (ui != null) ui.setOnAccessibilityEventListener(null);
        }
        finish(code, result);
    }
}
