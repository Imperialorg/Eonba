package com.eon.assistant;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Activity;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.content.ComponentName;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.service.voice.VoiceInteractionService;
import android.text.TextUtils;
import android.view.accessibility.AccessibilityNodeInfo;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

/** External observer; no grants, context injection, global window queries or permission dialogs. */
public final class ProfileActionAcceptance extends Instrumentation {
    private static final String APP = "com.eon.assistant";
    private UiAutomation ui;
    private volatile AccessibilityNodeInfo root;
    private String step = "preflight";
    private boolean denied;

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        denied = args != null && "denied".equals(args.getString("policy"));
        start();
    }

    private String shell(String command) throws Exception {
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(
                new ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command))))) {
            StringBuilder out = new StringBuilder();
            for (String s; (s = reader.readLine()) != null;) out.append(s).append('\n');
            return out.toString().trim();
        }
    }

    private static void require(boolean value) { if (!value) throw new AssertionError(); }

    private void bindingClosed() throws Exception {
        step = "active-service";
        require(VoiceInteractionService.isActiveService(getTargetContext(),
                new ComponentName(APP, APP + ".EonVoiceInteractionService")));
        step = "closed-collection";
        // UiAutomation executes an argv, not a shell pipeline. Filter inside the device shell.
        ParcelFileDescriptor[] pipes = ui.executeShellCommandRw("sh");
        try (BufferedReader fields = new BufferedReader(new InputStreamReader(
                new ParcelFileDescriptor.AutoCloseInputStream(pipes[0]), StandardCharsets.US_ASCII))) {
            try (ParcelFileDescriptor.AutoCloseOutputStream script = new ParcelFileDescriptor.AutoCloseOutputStream(pipes[1])) {
                script.write("dumpsys voiceinteraction | grep -E '^[[:space:]]*mDisabledShowContext='\n"
                        .getBytes(StandardCharsets.US_ASCII));
            }
            String field = fields.readLine();
            require(field != null && "mDisabledShowContext=3".equals(field.trim()) && fields.readLine() == null);
        }
    }

    private AccessibilityNodeInfo await(String id, String text) {
        step = id;
        long until = SystemClock.elapsedRealtime() + 12_000;
        while (SystemClock.elapsedRealtime() < until) {
            ui.clearCache();
            AccessibilityNodeInfo r = root;
            if (r != null && r.refresh() && TextUtils.equals(APP, r.getPackageName())) {
                for (AccessibilityNodeInfo n : r.findAccessibilityNodeInfosByViewId(APP + ":id/" + id)) {
                    if (n.getWindowId() == r.getWindowId() && TextUtils.equals(APP, n.getPackageName())
                            && n.isVisibleToUser() && (text == null || TextUtils.equals(text, n.getText()))) return n;
                }
            }
            SystemClock.sleep(80);
        }
        throw new AssertionError();
    }

    private void click(String id) {
        AccessibilityNodeInfo n = await(id, null);
        // A new activity can publish its nodes during the opening animation, before it
        // receives window focus. Never ask the production source to relax its focus guard.
        SystemClock.sleep(450);
        require(n.refresh() && n.performAction(AccessibilityNodeInfo.ACTION_CLICK));
    }

    @Override public void onStart() {
        int code = Activity.RESULT_CANCELED;
        Bundle result = new Bundle();
        try {
            require(Build.VERSION.SDK_INT == 35 && "ranchu".equals(Build.HARDWARE)
                    && (APP + ".keystoretest").equals(getTargetContext().getPackageName()));
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
            bindingClosed();
            step = "system-policy";
            require(denied == "0".equals(shell("settings get secure assist_structure_enabled")));
            AccessibilityServiceInfo info = ui.getServiceInfo();
            info.packageNames = new String[]{APP};
            info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            ui.setServiceInfo(info);
            ui.setOnAccessibilityEventListener(event -> {
                if (!TextUtils.equals(APP, event.getPackageName())) return;
                AccessibilityNodeInfo n = event.getSource();
                for (int depth = 0; depth < 24 && n != null && TextUtils.equals(APP, n.getPackageName()); depth++) {
                    AccessibilityNodeInfo p = n.getParent();
                    if (p == null) { root = n; return; }
                    n = p;
                }
            });
            shell("am start -f 0x14000000 -n com.eon.assistant/.AssistantActivity");
            click("profile_action_open");
            require(await("profile_action_status", null).getText().toString().startsWith("Allow the selected EON assistant"));
            shell("input keyevent BACK");
            click("profile_action_open");
            click("profile_action_confirm");
            await("hud_context", denied ? "No authorized Android direct action result."
                    : "Android direct action completed. Suggested model profile: p0_mobile_ultralight");
            bindingClosed();
            click("hud_end");
            String source = await("profile_action_status", null).getText().toString();
            require(denied ? !source.equals("This page ran the approved profile check.")
                    : source.equals("This page ran the approved profile check."));
            shell("input keyevent KEYCODE_ASSIST");
            await("hud_status", "Session open");
            require(root.findAccessibilityNodeInfosByViewId(APP + ":id/hud_context").stream()
                    .noneMatch(AccessibilityNodeInfo::isVisibleToUser));
            click("hud_end");
            bindingClosed();
            code = Activity.RESULT_OK;
            result.putString("profileAction", denied ? "PASS-policy-denied" : "PASS-os-action");
        } catch (Exception | AssertionError | LinkageError e) {
            result.putString("profileAction", "FAIL-" + step);
        }
        finish(code, result);
    }
}
