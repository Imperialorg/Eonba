package com.eon.assistant

import android.app.Activity
import android.app.ActivityManager
import android.app.DirectAction
import android.os.Binder
import android.os.Bundle
import android.os.CancellationSignal
import android.os.SystemClock
import android.service.voice.VoiceInteractionSession
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.util.function.Consumer
import uniffi.eon_uniffi.selectModelProfile

/** Private, non-editable source of one read-only DirectAction. No intent-supplied state. */
class ProfileActionActivity : Activity() {
    private lateinit var status: TextView
    private var resumed = false
    internal var review: ProfileActionReview? = null
        private set

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        window.setHideOverlayWindows(true)
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            fitsSystemWindows = true
            importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            status = TextView(context).apply {
                id = R.id.profile_action_status
                setText(R.string.profile_action_review)
            }.also(::addView)
            addView(Button(context).apply {
                id = R.id.profile_action_confirm
                setText(R.string.profile_action_confirm)
                filterTouchesWhenObscured = true
                setOnClickListener { confirm() }
            })
        })
    }

    internal fun foreground() = resumed && hasWindowFocus() && !isInMultiWindowMode &&
        !isInPictureInPictureMode && selectedAndUnlocked(this)

    private fun confirm() {
        if (review != null) return
        val binding = voiceHud.binding
        if (binding == null || !foreground() || !isLocalVoiceInteractionSupported || isVoiceInteraction) {
            status.setText(R.string.profile_action_unavailable)
            return
        }
        val next = ProfileActionReview(this, binding)
        review = next
        try {
            startLocalVoiceInteraction(next.issue())
            status.postDelayed({
                if (review === next && !next.presenting) {
                    cancel()
                    status.setText(R.string.profile_action_unavailable)
                }
            }, ProfileActionReview.TIMEOUT)
        } catch (_: RuntimeException) {
            cancel()
            status.setText(R.string.profile_action_unavailable)
        }
    }

    override fun onLocalVoiceInteractionStarted() {
        super.onLocalVoiceInteractionStarted()
        val r = review
        val interactor = voiceInteractor
        if (r == null || !r.valid() || interactor?.packageName != packageName) {
            stopLocalVoiceInteraction()
            return
        }
        // The OS inserts this activity's own task ID and assist token into the notification.
        r.notified = true
        interactor.notifyDirectActionsChanged()
    }

    override fun onGetDirectActions(stop: CancellationSignal, callback: Consumer<MutableList<DirectAction>>) {
        val r = review
        if (r == null || !r.discovering || r.offered || r.permitted?.invoke() != true || stop.isCanceled ||
            voiceInteractor?.packageName != packageName) {
            callback.accept(mutableListOf())
            return
        }
        r.offered = true
        callback.accept(mutableListOf(DirectAction.Builder(ProfileActionReview.ACTION).build()))
    }

    override fun onPerformDirectAction(id: String, args: Bundle, stop: CancellationSignal, callback: Consumer<Bundle>) {
        val r = review
        val result = Bundle()
        if (id == ProfileActionReview.ACTION && r != null && r.performing && !r.completed && r.permitted?.invoke() == true &&
            !stop.isCanceled && voiceInteractor?.packageName == packageName && args.getBinder(ProfileActionReview.KEY) === r.token) {
            r.completed = true // consume before executing the single fixed operation
            try {
                val memory = ActivityManager.MemoryInfo()
                getSystemService(ActivityManager::class.java).getMemoryInfo(memory)
                result.putString(ProfileActionReview.RESULT, selectModelProfile(memory.totalMem / 1_000_000_000.0))
                status.setText(R.string.profile_action_done)
            } catch (_: Exception) {
                status.setText(R.string.profile_action_unavailable)
            } catch (_: LinkageError) {
                status.setText(R.string.profile_action_unavailable)
            }
        }
        callback.accept(result)
    }

    private fun cancel() {
        val old = review ?: return
        review = null
        old.forget()
        if (isVoiceInteraction) stopLocalVoiceInteraction()
    }

    override fun onLocalVoiceInteractionStopped() {
        review?.forget()
        review = null
        super.onLocalVoiceInteractionStopped()
    }
    override fun onWindowFocusChanged(focus: Boolean) {
        super.onWindowFocusChanged(focus)
        if (!focus && review?.presenting != true) cancel()
    }
    override fun onResume() { super.onResume(); resumed = true }
    override fun onPause() { resumed = false; cancel(); super.onPause() }
}

internal class ProfileActionReview(val source: ProfileActionActivity, private val binding: VoiceHudRegistry.Binding) {
    val token = Binder()
    private val deadline = SystemClock.elapsedRealtime() + TIMEOUT
    var notified = false
    var permitted: (() -> Boolean)? = null
    var discovering = false
    var offered = false
    var performing = false
    var completed = false
    var presenting = false
    fun valid() = source.review === this && voiceHud.binding === binding &&
        SystemClock.elapsedRealtime() < deadline && source.foreground()
    fun issue() = Bundle().apply { pending = this@ProfileActionReview; putBinder(KEY, token) }
    fun forget() { if (pending === this) pending = null }
    companion object {
        const val TIMEOUT = 5_000L
        const val KEY = "com.eon.assistant.PROFILE_AUTHORIZATION"
        const val ACTION = "com.eon.assistant.CHECK_MODEL_PROFILE"
        const val RESULT = "profile"
        private var pending: ProfileActionReview? = null
        // Deliberately refuse to reuse a session with assist callbacks from an earlier showing.
        fun candidate() = pending
        fun take(candidate: ProfileActionReview?, flags: Int, args: Bundle?): ProfileActionReview? {
            val r = pending ?: return null
            pending = null
            return try {
                r.takeIf { r === candidate && flags == VoiceInteractionSession.SHOW_SOURCE_ACTIVITY &&
                    r.valid() && args?.getBinder(KEY) === r.token }
            } catch (_: RuntimeException) { null }
        }
    }
}
