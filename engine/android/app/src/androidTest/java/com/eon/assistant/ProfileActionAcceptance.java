package com.eon.assistant;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Activity;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.app.admin.DevicePolicyManager;
import android.content.ComponentName;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.os.UserManager;
import android.service.voice.VoiceInteractionService;
import android.text.TextUtils;
import android.view.accessibility.AccessibilityNodeInfo;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

/** External observer; no grants, context injection, global window queries or permission dialogs. */
public final class ProfileActionAcceptance extends Instrumentation {
    private static final String APP = "com.eon.assistant";
    private static final String COMPLETED =
            "Android direct action completed. Suggested model profile: p0_mobile_ultralight";
    private static final String SOURCE_DONE = "This page ran the approved profile check.";
    private UiAutomation ui;
    private volatile AccessibilityNodeInfo root;
    private String step = "preflight";
    private String policy;
    private String deathPhase;
    private boolean validArgs;

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        policy = args == null ? "allowed" : args.getString("policy", "allowed");
        deathPhase = args == null ? null : args.getString("deathPhase");
        validArgs = args == null || args.isEmpty() || (args.size() == 1 &&
                (args.containsKey("policy") || "arm".equals(deathPhase) || "recover".equals(deathPhase)));
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

    private static AccessibilityNodeInfo ownedRoot(AccessibilityNodeInfo n) {
        for (int depth = 0; depth < 24 && n != null && TextUtils.equals(APP, n.getPackageName()); depth++) {
            AccessibilityNodeInfo p = n.getParent(0);
            if (p == null) return n;
            if (p.getWindowId() != n.getWindowId()) return null;
            n = p;
        }
        return null;
    }

    private AccessibilityNodeInfo await(String id, String text) {
        step = id.replace('_', '-');
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

    private AccessibilityNodeInfo unconfirmedPage() {
        AccessibilityNodeInfo n = await("profile_action_status", null);
        require(n.getText().toString().startsWith("Allow the selected EON assistant"));
        return n;
    }

    // No death command here. Only the separately opted-in host may crash the matched package.
    private void observeDeathPhase() throws Exception {
        click("profile_action_open");
        int sourceWindow = unconfirmedPage().getWindowId();
        if ("recover".equals(deathPhase)) {
            bindingClosed();
            shell("input keyevent KEYCODE_ASSIST"); // over the private page, WITHOUT confirming
            AccessibilityNodeInfo status = await("hud_status", "Session open");
            require(status.getWindowId() != sourceWindow);
            AccessibilityNodeInfo card = ownedRoot(status);
            step = "unapproved-card";
            // Observe the live card, not an absent/stale window, for a whole ticket lifetime.
            long until = SystemClock.elapsedRealtime() + 5_500;
            do {
                ui.clearCache();
                require(card != null && card.refresh() && TextUtils.equals(APP, card.getPackageName())
                        && card.getWindowId() == status.getWindowId() && status.refresh()
                        && status.isVisibleToUser() && TextUtils.equals("Session open", status.getText()));
                require(card.findAccessibilityNodeInfosByViewId(APP + ":id/hud_context").stream()
                        .noneMatch(AccessibilityNodeInfo::isVisibleToUser));
                SystemClock.sleep(100);
            } while (SystemClock.elapsedRealtime() < until);
            click("hud_end");
            require(unconfirmedPage().getWindowId() == sourceWindow);
        }
        click("profile_action_confirm");
        await("hud_context", COMPLETED);
        bindingClosed();
        if ("recover".equals(deathPhase)) {
            click("hud_end");
            await("profile_action_status", SOURCE_DONE);
        } // arm leaves the authorized result visible for the host, without dismissing it
    }

    @Override public void onStart() {
        int code = Activity.RESULT_CANCELED;
        Bundle result = new Bundle();
        try {
            require(validArgs && Build.VERSION.SDK_INT == 35 && "ranchu".equals(Build.HARDWARE)
                    && (APP + ".keystoretest").equals(getTargetContext().getPackageName()));
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
            bindingClosed();
            step = "system-policy";
            require("allowed".equals(policy) || "denied".equals(policy) || "restricted".equals(policy)
                    || "capture-disabled".equals(policy));
            boolean denied = !"allowed".equals(policy);
            // Each negative case requires its own real policy to be the only blocker.
            String textSwitch = shell("settings --user current get secure assist_structure_enabled");
            // Android defaults this switch to enabled when the setting is absent.
            require("0".equals(textSwitch) || "1".equals(textSwitch) || "null".equals(textSwitch));
            require("denied".equals(policy) == "0".equals(textSwitch));
            require("restricted".equals(policy) == getTargetContext().getSystemService(UserManager.class)
                    .hasUserRestriction(UserManager.DISALLOW_ASSIST_CONTENT));
            require("capture-disabled".equals(policy) == getTargetContext().getSystemService(DevicePolicyManager.class)
                    .getScreenCaptureDisabled(null));
            AccessibilityServiceInfo info = ui.getServiceInfo();
            info.packageNames = new String[]{APP};
            info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            ui.setServiceInfo(info);
            ui.setOnAccessibilityEventListener(event -> {
                if (!TextUtils.equals(APP, event.getPackageName())) return;
                AccessibilityNodeInfo r = ownedRoot(event.getSource(0));
                if (r != null) root = r;
            });
            shell("am start -f 0x14000000 -n com.eon.assistant/.AssistantActivity");
            if (deathPhase != null) {
                observeDeathPhase();
                result.putString("profileDeath", "arm".equals(deathPhase) ? "ARMED-visible-action" : "PASS-fresh-consent");
                finish(Activity.RESULT_OK, result);
                return;
            }
            click("profile_action_open");
            require(await("profile_action_status", null).getText().toString().startsWith("Allow the selected EON assistant"));
            shell("input keyevent BACK");
            click("profile_action_open");
            click("profile_action_confirm");
            await("hud_context", denied ? "No authorized Android direct action result."
                    : COMPLETED);
            bindingClosed();
            click("hud_end");
            String source = await("profile_action_status", null).getText().toString();
            require(denied ? source.startsWith("Allow the selected EON assistant")
                    : source.equals(SOURCE_DONE));
            // The second context route is a user-volunteered setting, not a direct action.
            shell("am start -f 0x14000000 -n com.eon.assistant/.AssistantActivity");
            click("assistant_context_review");
            require(await("assistant_context_review", null).getText().toString().regionMatches(
                    true, 0, "Share once: CPU timing is OFF.", 0, "Share once: CPU timing is OFF.".length()));
            click("assistant_context_review");
            await("hud_context", denied
                    ? "Shared setting unavailable. Screen layout and screenshot collection are always off."
                    : "EON controls shared: CPU timing OFF. Check model profile in controls without measuring this process. No screen contents collected.");
            click("hud_end");
            shell("input keyevent KEYCODE_ASSIST");
            await("hud_status", "Session open");
            require(root.findAccessibilityNodeInfosByViewId(APP + ":id/hud_context").stream()
                    .noneMatch(AccessibilityNodeInfo::isVisibleToUser));
            click("hud_end");
            bindingClosed();
            code = Activity.RESULT_OK;
            result.putString("profileAction", denied ? "PASS-policy-" + policy : "PASS-os-action");
        } catch (Exception | AssertionError | LinkageError e) {
            result.putString(deathPhase == null ? "profileAction" : "profileDeath", "FAIL-" + step);
        }
        finish(code, result);
    }
}
