package com.eon.assistant

import android.app.Activity
import android.app.KeyguardManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/** Private API 34+ demo. Neither intents nor the accessibility service supply actions or targets. */
class DemoActionActivity : Activity() {
    private lateinit var targets: LinearLayout
    private lateinit var gate: DemoActionGate<View>
    private lateinit var status: TextView
    private lateinit var confirm: Button
    private lateinit var cancel: Button
    private val handler = Handler(Looper.getMainLooper())
    private val expire = Runnable { clearReview("Review expired. No action was taken.") }
    private var resumed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT < 34) { finish(); return }
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        window.setHideOverlayWindows(true)
        title = "EON accessibility action demo"
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val pad = dp(12)
            setPadding(pad, pad, pad, pad)
            fitsSystemWindows = true
            isSaveFromParentEnabled = false
        }
        val controls = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        controls.addView(TextView(this).apply {
            text = "Review and confirm one action on the sample controls below. The accessibility shortcut only inspects; it never confirms an action."
        })
        targets = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        var count = 0
        val counter = Button(this).apply {
            id = R.id.demo_counter
            text = "Demo count: 0"
            setOnClickListener { text = "Demo count: ${++count}" }
        }
        val samples = ScrollView(this).apply {
            id = R.id.demo_samples
            addView(LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                for (i in 1..16) addView(TextView(context).apply {
                    text = "Sample row $i"
                    minHeight = dp(36)
                })
            })
        }
        targets.addView(counter)
        targets.addView(samples, -1, dp(104))
        column.addView(targets)
        gate = DemoActionGate<View>(counter, samples, targets::getChildAt, ::supports,
            ::liveScope, SystemClock::elapsedRealtime) { node, action ->
            node.performAccessibilityAction(action.nativeId(), null)
        }
        controls.addView(reviewButton(R.id.demo_review_click, "Review counter click", DemoAction.CLICK_COUNTER))
        controls.addView(reviewButton(R.id.demo_review_scroll, "Review list scroll", DemoAction.SCROLL_SAMPLES_FORWARD))
        status = TextView(this).apply { accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE }
        controls.addView(status)
        confirm = protectedButton().apply { id = R.id.demo_confirm }
        cancel = protectedButton().apply {
            id = R.id.demo_cancel
            text = "Cancel"
            setOnClickListener { clearReview("Canceled. No action was taken.") }
        }
        controls.addView(confirm)
        controls.addView(cancel)
        clearReview("No action is approved.")
        column.addView(ScrollView(this).apply { addView(controls) }, LinearLayout.LayoutParams(-1, 0, 1f))
        setContentView(column)
    }

    private fun dp(value: Int) = (value * resources.displayMetrics.density + 0.5f).toInt()

    private fun protectedButton(): Button = object : Button(this) {
        init {
            filterTouchesWhenObscured = true
            setAccessibilityDataSensitive(View.ACCESSIBILITY_DATA_SENSITIVE_YES)
        }
        override fun onFilterTouchEventForSecurity(event: MotionEvent): Boolean =
            event.flags and MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED == 0 && super.onFilterTouchEventForSecurity(event)
    }

    private fun reviewButton(id: Int, label: String, action: DemoAction) = protectedButton().also {
        it.id = id
        it.text = label
        it.setOnClickListener { review(action) }
    }

    private fun liveScope(): Any? {
        if (!resumed || isFinishing || isDestroyed || !hasWindowFocus() || isInMultiWindowMode ||
            isInPictureInPictureMode || !targets.isAttachedToWindow || targets.rootView !== window.decorView ||
            !targets.isShown || getSystemService(KeyguardManager::class.java)?.isDeviceLocked != false ||
            !InspectionAccess.eligible(this)) return null
        return InspectionAccess.environmentEpoch
    }

    // Read only capability metadata on one known, same-window View. Never enumerate OS windows,
    // accessibility roots, node text, other package controls, or children of unknown views.
    private fun supports(view: View, action: DemoAction): Boolean {
        if (view.context !== this || view.parent !== targets || view.windowToken !== window.decorView.windowToken ||
            !view.isAttachedToWindow || !view.isShown || !view.isEnabled || view.isAccessibilityDataSensitive) return false
        val node = view.createAccessibilityNodeInfo() ?: return false
        return node.packageName == packageName && node.isVisibleToUser && node.isEnabled &&
            !node.isAccessibilityDataSensitive && !node.isPassword && !node.isEditable && node.inputType == 0 &&
            node.actionList.any { it.id == action.nativeId() }
    }

    private fun DemoAction.nativeId(): Int = when (this) {
        DemoAction.CLICK_COUNTER -> AccessibilityNodeInfo.ACTION_CLICK
        DemoAction.SCROLL_SAMPLES_FORWARD -> AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
    }

    private fun review(action: DemoAction) {
        clearReview("")
        val request = try { gate.review(action) } catch (_: RuntimeException) { null }
        if (request == null) {
            status.text = "Unavailable. Unlock the device, use full screen, select EON as assistant, and enable its accessibility service and shortcut. The sample control must be visible and support the action."
            return
        }
        confirm.text = when (action) {
            DemoAction.CLICK_COUNTER -> "Confirm: increment demo count once"
            DemoAction.SCROLL_SAMPLES_FORWARD -> "Confirm: scroll sample list forward once"
        }
        status.text = "Review expires in 15 seconds. Nothing has happened yet."
        confirm.setOnClickListener {
            val ok = try { gate.confirm(request) } catch (_: RuntimeException) { false }
            clearReview(if (ok) "Demo action performed. No approval remains." else "Action unavailable. Review again.")
        }
        confirm.visibility = View.VISIBLE
        cancel.visibility = View.VISIBLE
        handler.postDelayed(expire, DemoActionGate.REVIEW_MS)
    }

    private fun clearReview(message: String) {
        if (!::gate.isInitialized) return
        gate.cancel()
        handler.removeCallbacks(expire)
        confirm.setOnClickListener(null)
        confirm.visibility = View.GONE
        cancel.visibility = View.GONE
        status.text = message
    }

    override fun onResume() { super.onResume(); resumed = true }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus) clearReview("Review cleared. No action is approved.")
    }
    override fun onPause() {
        resumed = false
        clearReview("Review cleared. No action is approved.")
        super.onPause()
    }
}
