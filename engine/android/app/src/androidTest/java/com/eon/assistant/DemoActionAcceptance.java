package com.eon.assistant;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.view.accessibility.AccessibilityNodeInfo;
import android.widget.Button;
import android.widget.ScrollView;
import android.widget.TextView;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;

/**
 * Same-signer device test, built by keystore_device_test.py --build-only. On a disposable,
 * unlocked API 34+ device install both APKs, then run in full screen:
 * adb shell am instrument -w -e mode denied com.eon.assistant.keystoretest/com.eon.assistant.DemoActionAcceptance
 * Repeat with "allowed" after the operator selects EON as assistant and enables its service
 * and shortcut. The test drives only the demo's own buttons. It never grants a role, injects
 * input coordinates, opens settings, or substitutes test-only eligibility in the app.
 */
public final class DemoActionAcceptance extends Instrumentation {
    private String mode;
    private DemoActionActivity activity;
    private View cover;
    private int nativeCalls;

    @Override public void onCreate(Bundle args) {
        super.onCreate(args);
        mode = args == null ? null : args.getString("mode");
        start();
    }

    private static void check(boolean ok) {
        if (!ok) throw new AssertionError("demo assertion failed");
    }

    private <T> T main(Callable<T> read) throws Exception {
        FutureTask<T> task = new FutureTask<>(read);
        runOnMainSync(task);
        return task.get();
    }

    private void await(Callable<Boolean> condition) throws Exception {
        long start = SystemClock.elapsedRealtime();
        while (!main(condition)) {
            check(SystemClock.elapsedRealtime() - start < 5000L);
            SystemClock.sleep(25L);
        }
    }

    private Button button(int id) { return activity.findViewById(id); }
    private void tap(int id) {
        Button button = button(id);
        check(button.isShown() && button.isEnabled() && button.performClick());
    }
    private String count() { return button(R.id.demo_counter).getText().toString(); }
    private ScrollView samples() { return activity.findViewById(R.id.demo_samples); }
    private String status() { return ((TextView) activity.findViewById(R.id.demo_status)).getText().toString(); }
    private boolean reviewing() { return button(R.id.demo_confirm).getVisibility() == View.VISIBLE; }
    private void reviewLocal(int id) {
        tap(id);
        check(reviewing() && button(R.id.demo_confirm).getText().toString().contains("local hit fallback"));
    }

    @Override public void onStart() {
        String step = "startup";
        Bundle result = new Bundle();
        int code = Activity.RESULT_CANCELED;
        try {
            check(Build.VERSION.SDK_INT >= 34 && ("denied".equals(mode) || "allowed".equals(mode)));
            activity = (DemoActionActivity) startActivitySync(new Intent(getTargetContext(), DemoActionActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            step = "foreground";
            await(activity::hasWindowFocus);
            if ("denied".equals(mode)) {
                step = "authority-denied";
                main(() -> {
                    check(!InspectionAccess.INSTANCE.eligible(activity));
                    for (int i = 0; i < 2; i++) {
                        if (i == 1) tap(R.id.demo_sample_mode);
                        for (int review : new int[]{R.id.demo_review_click, R.id.demo_review_scroll}) {
                            tap(review);
                            check(!reviewing());
                            button(R.id.demo_confirm).performClick();
                            check("Demo count: 0".equals(count()) && samples().getScrollY() == 0);
                        }
                    }
                    return null;
                });
            } else {
                step = "operator-prerequisites";
                await(() -> InspectionAccess.INSTANCE.eligible(activity));
                step = "cancel-and-click";
                main(() -> {
                    tap(R.id.demo_review_click);
                    check(reviewing() && "Demo count: 0".equals(count()));
                    tap(R.id.demo_cancel);
                    check(!reviewing() && "Demo count: 0".equals(count()));
                    tap(R.id.demo_review_click);
                    tap(R.id.demo_confirm);
                    check(!reviewing() && "Demo count: 1".equals(count()));
                    check(status().startsWith("Verified:") && status().contains("native action"));
                    button(R.id.demo_confirm).performClick();
                    check("Demo count: 1".equals(count()));
                    return null;
                });
                step = "changed-target";
                main(() -> {
                    tap(R.id.demo_review_click);
                    button(R.id.demo_counter).setEnabled(false);
                    tap(R.id.demo_confirm);
                    check(!reviewing() && "Demo count: 1".equals(count()));
                    button(R.id.demo_counter).setEnabled(true);
                    return null;
                });
                step = "native-scroll";
                main(() -> {
                    tap(R.id.demo_review_scroll);
                    check(reviewing() && samples().getScrollY() == 0);
                    tap(R.id.demo_confirm);
                    return null;
                });
                await(() -> samples().getScrollY() > 0 && status().startsWith("Verified:") && status().contains("native action"));
                step = "local-cancel-and-state-change";
                main(() -> {
                    tap(R.id.demo_review_click);
                    check(reviewing());
                    tap(R.id.demo_sample_mode);
                    check(!reviewing() && "Demo count: 1".equals(count()));
                    reviewLocal(R.id.demo_review_click);
                    tap(R.id.demo_cancel);
                    button(R.id.demo_confirm).performClick();
                    check(!reviewing() && "Demo count: 1".equals(count()));
                    reviewLocal(R.id.demo_review_scroll);
                    samples().scrollBy(0, 7); // Still the same ScrollView; the reviewed state is stale.
                    int moved = samples().getScrollY();
                    tap(R.id.demo_confirm);
                    check(!reviewing() && samples().getScrollY() == moved && !status().startsWith("Verified:"));
                    return null;
                });
                step = "new-overlapping-sibling";
                main(() -> {
                    reviewLocal(R.id.demo_review_click);
                    cover = new View(activity);
                    ((ViewGroup) activity.findViewById(android.R.id.content)).addView(cover, -1, -1);
                    return null;
                });
                await(() -> cover.isLaidOut() && cover.getHeight() > 0);
                main(() -> {
                    tap(R.id.demo_confirm);
                    check(!reviewing() && "Demo count: 1".equals(count()) && !status().startsWith("Verified:"));
                    ((ViewGroup) cover.getParent()).removeView(cover);
                    return null;
                });
                step = "local-click";
                main(() -> {
                    reviewLocal(R.id.demo_review_click);
                    tap(R.id.demo_confirm);
                    check(status().startsWith("Verified:") && status().contains("local hit fallback"));
                    button(R.id.demo_confirm).performClick();
                    check(!reviewing() && "Demo count: 2".equals(count()));
                    return null;
                });
                step = "local-scroll";
                main(() -> {
                    int before = samples().getScrollY();
                    reviewLocal(R.id.demo_review_scroll);
                    tap(R.id.demo_confirm);
                    int after = samples().getScrollY();
                    check(after > before && after - before <= samples().getHeight());
                    check(status().startsWith("Verified:") && status().contains("local hit fallback"));
                    button(R.id.demo_confirm).performClick();
                    check(!reviewing() && samples().getScrollY() == after);
                    return null;
                });
                step = "ambiguous-native-result";
                main(() -> {
                    tap(R.id.demo_sample_mode);
                    button(R.id.demo_counter).setAccessibilityDelegate(new View.AccessibilityDelegate() {
                        @Override public boolean performAccessibilityAction(View host, int id, Bundle args) {
                            if (id != AccessibilityNodeInfo.ACTION_CLICK) return super.performAccessibilityAction(host, id, args);
                            nativeCalls++;
                            host.performClick(); // Effect occurred despite a false provider return.
                            return false;
                        }
                    });
                    tap(R.id.demo_review_click);
                    check(reviewing() && button(R.id.demo_confirm).getText().toString().contains("native action"));
                    tap(R.id.demo_confirm);
                    button(R.id.demo_confirm).performClick();
                    check(!reviewing() && !status().startsWith("Verified:") && "Demo count: 3".equals(count()) && nativeCalls == 1);
                    return null;
                });
            }
            result.putString("demo", "PASS-" + mode);
            code = Activity.RESULT_OK;
        } catch (Exception | AssertionError | LinkageError failure) {
            result.putString("demo", "FAIL-" + step);
        } finally {
            if (activity != null) runOnMainSync(activity::finish);
            finish(code, result);
        }
    }
}
