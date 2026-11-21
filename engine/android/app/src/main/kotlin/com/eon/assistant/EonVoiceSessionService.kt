package com.eon.assistant

import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import android.view.View
import android.view.WindowManager
import android.widget.TextView
import java.util.concurrent.Callable
import java.util.concurrent.ExecutionException
import java.util.concurrent.FutureTask
import uniffi.eon_uniffi.runProfileWorkflow

class EonVoiceSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = EonVoiceSession(this)
}

internal fun userInvokedVoiceHud(flags: Int): Boolean = when (flags and NO_ASSIST_CONTEXT.inv()) {
    VoiceInteractionSession.SHOW_SOURCE_ASSIST_GESTURE, VoiceInteractionSession.SHOW_SOURCE_PUSH_TO_TALK -> true
    else -> false
}

private class EonVoiceSession(context: Context) : VoiceInteractionSession(context) {
    private val surface = voiceHud.Surface(voiceHud.binding, ::closeHud)
    private val main = Handler(Looper.getMainLooper())
    private var hud: VoiceHudView? = null
    private var work: FutureTask<ProfileCheck.Complete>? = null
    private var toneStop: CancellationSignal? = null
    private var taskStop: CancellationSignal? = null
    private var microphone: SessionMic? = null
    private var contextTicket: ControlsConsent.Ticket? = null
    private var contextDisplay: ControlsConsent.Ticket? = null
    private var contextNotice: String? = null
    private val contextGuard = object : Runnable {
        override fun run() {
            if (contextNotice == null || !requireVisible()) return
            if (!contextSharingEnabled() || contextDisplay?.let { !ControlsAssist.mayDisplay(it) } == true)
                return unavailableContext()
            val ticket = contextTicket
            if (ticket != null && android.os.SystemClock.elapsedRealtime() >= ticket.until) {
                unavailableContext()
                return
            }
            main.postDelayed(this, 300)
        }
    }
    private var micLevels = emptyList<Int>()
    private val micGuard = object : Runnable {
        override fun run() {
            if (microphone == null) return
            if (!micEligible()) clearMic(R.string.voice_mic_failed)
            else main.postDelayed(this, 100)
        }
    }
    private var observingScreenOff = false
    private val screenOff = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_SCREEN_OFF) closeHud()
        }
    }

    private fun eligible(): Boolean {
        return try {
            selectedAndUnlocked(context) &&
                disabledShowContext and NO_ASSIST_CONTEXT == NO_ASSIST_CONTEXT
        } catch (_: SecurityException) {
            false
        }

    }

    private fun contextSharingEnabled() = try {
        userDisabledShowContext and SHOW_WITH_ASSIST == 0
    } catch (_: SecurityException) {
        false
    }

    override fun onPrepareShow(args: Bundle?, showFlags: Int) {
        clearShowing()
        val ticket = ControlsAssist.beginShow(args, showFlags)
        val allowed = surface.show(ticket != null || userInvokedVoiceHud(showFlags), eligible())
        if (allowed && ticket != null) {
            contextTicket = ticket
            contextNotice = context.getString(R.string.voice_context_waiting)
        } else ControlsAssist.cancel(ticket)
        setUiEnabled(allowed)
    }

    override fun onShow(args: Bundle?, showFlags: Int) {
        if (!requireVisible()) return
        FloatingHudService.stopFrom(context)
        InspectionAccess.cancel()
        try {
            // SCREEN_OFF is a protected system broadcast. Observe only while this card is shown.
            context.applicationContext.registerReceiver(screenOff, IntentFilter(Intent.ACTION_SCREEN_OFF))
            observingScreenOff = true
        } catch (_: SecurityException) {
            closeHud()
            return
        }
        if (requireVisible()) {
            render()
            if (contextTicket != null) main.post(contextGuard)
        }
    }

    override fun onHandleAssist(state: AssistState) {
        val ticket = contextTicket ?: return
        if (!requireVisible()) return
        if (!contextSharingEnabled()) return unavailableContext()
        val count = ControlsAssist.protectedWindows(context, ticket, state)
        if (count == null) return unavailableContext()
        contextTicket = null
        contextDisplay = ticket
        contextNotice = context.getString(R.string.voice_context_protected, count)
        render()
    }
    override fun onHandleScreenshot(screenshot: Bitmap?) = Unit

    private fun discardContext() {
        ControlsAssist.cancel(contextTicket)
        ControlsAssist.cancel(contextDisplay)
        contextTicket = null
        contextDisplay = null
        contextNotice = null
        main.removeCallbacks(contextGuard)
    }

    private fun unavailableContext() {
        discardContext()
        contextNotice = context.getString(R.string.voice_context_unavailable)
        render()
    }

    override fun onCreateContentView(): View {
        window?.window?.apply {
            addFlags(WindowManager.LayoutParams.FLAG_SECURE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL)
            clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
            setBackgroundDrawableResource(android.R.color.transparent)
            setHideOverlayWindows(true)
        }
        return VoiceHudView(context, ::toggleDetails, ::checkProfile, ::sampleTasks, ::analyzeTone, ::clearTone,
            ::openControls, ::closeHud).also {
            hud = it
            it.setMicClick(::toggleMic)
            it.onMicWindowLost = { clearMic() }
            render()
        }
    }

    override fun onComputeInsets(outInsets: Insets) {
        // Leave the app underneath at its original size; only the floating card takes touches.
        outInsets.contentInsets.set(0, hud?.rootView?.height ?: 0, 0, 0)
        outInsets.touchableInsets = Insets.TOUCHABLE_INSETS_REGION
        outInsets.touchableRegion.setEmpty()
        if (surface.frame != null) hud?.fillTouchableRegion(outInsets.touchableRegion)
    }

    private fun render() {
        hud?.render(surface.frame)
        hud?.findViewById<TextView>(R.id.hud_context)?.apply {
            text = contextNotice
            visibility = if (contextNotice == null) View.GONE else View.VISIBLE
        }
    }

    private fun requireVisible(): Boolean {
        if (surface.canUse(eligible())) return true
        closeHud()
        return false
    }

    private fun toggleDetails() {
        if (!requireVisible()) return
        if (surface.frame?.expanded == true) {
            clearMic()
            clearTone()
            cancelTaskSample()
        }
        surface.toggleDetails()
        render()
    }

    private fun micEligible() = surface.frame?.expanded == true && hud?.micVisible() == true &&
        surface.canUse(eligible()) && MicPermission.allowed(context)

    private fun toggleMic() {
        if (microphone != null) {
            clearMic(R.string.voice_mic_stopped)
            return
        }
        if (!requireVisible() || surface.frame?.expanded != true || hud?.micVisible() != true) return
        if (!MicPermission.allowed(context)) {
            clearMic(R.string.voice_mic_denied)
            return
        }
        val take = try { SessionMic.open() } catch (_: RuntimeException) {
            clearMic(R.string.voice_mic_failed)
            return
        }
        microphone = take
        micLevels = emptyList()
        hud?.renderMic(true, micLevels, context.getString(R.string.voice_mic_starting))
        main.post(micGuard)
        val app = context.applicationContext
        Thread({
            val ok = take.measure(app) { level ->
                main.post {
                    if (microphone !== take) return@post
                    if (!micEligible()) clearMic(R.string.voice_mic_failed)
                    else {
                        micLevels = micLevels + level
                        hud?.renderMic(true, micLevels, context.getString(R.string.voice_mic_progress, micLevels.size, level))
                    }
                }
            }
            main.post {
                if (microphone === take) {
                    if (ok && micLevels.size == ToneLevels.LIMIT && micEligible()) {
                        microphone = null // worker already released AudioRecord
                        main.removeCallbacks(micGuard)
                        hud?.renderMic(false, micLevels, context.getString(R.string.voice_mic_complete, micLevels.max()))
                    } else clearMic(R.string.voice_mic_failed)
                }
            }
        }, "EON visible microphone meter").start()
    }

    private fun clearMic(message: Int? = null) {
        val old = microphone
        microphone = null
        old?.close()
        main.removeCallbacks(micGuard)
        micLevels = emptyList()
        hud?.renderMic(false, micLevels, message?.let(context::getString))
    }

    private fun sampleTasks() {
        if (!requireVisible()) return
        val request = surface.requestTasks() ?: return
        val stop = CancellationSignal().also { taskStop = it }
        render()
        val app = context.applicationContext
        Thread({
            val counts = try { OwnActivityTasks.read(app, stop) } catch (_: RuntimeException) { null }
            main.post {
                if (taskStop === stop && requireVisible()) {
                    taskStop = null
                    surface.finishTasks(request, counts)
                    render()
                }
            }
        }, "EON activity tasks").start()
    }

    private fun cancelTaskSample() {
        taskStop?.cancel()
        taskStop = null
    }

    private fun analyzeTone() {
        if (!requireVisible()) return
        val request = surface.requestTone() ?: return
        val stop = CancellationSignal()
        toneStop = stop
        render()
        val app = context.applicationContext
        Thread({
            val ok = try {
                StockToneReader.read(app, stop) { level ->
                    main.post {
                        if (toneStop === stop && requireVisible()) {
                            surface.addLevel(request, level)
                            render()
                        }
                    }
                }
                true
            } catch (_: Exception) {
                false
            }
            main.post {
                if (toneStop === stop && requireVisible()) {
                    toneStop = null
                    surface.finishTone(request, ok)
                    render()
                }
            }
        }, "EON stock tone").start()
    }

    private fun clearTone() {
        val old = toneStop
        toneStop = null
        old?.cancel()
        surface.clearTone()
        render()
    }

    private fun checkProfile() {
        if (!requireVisible()) return
        val request = surface.requestCheck() ?: return
        render()
        val app = context.applicationContext
        val task = object : FutureTask<ProfileCheck.Complete>(Callable {
            val memory = ActivityManager.MemoryInfo()
            app.getSystemService(ActivityManager::class.java).getMemoryInfo(memory)
            val receipt = runProfileWorkflow(memory.totalMem / 1_000_000_000.0)
            ProfileCheck.Complete(receipt.profile, receipt.completedTasks)
        }) {
            override fun done() {
                if (isCancelled) return
                main.post {
                    if (surface.frame?.check !== request) return@post
                    work = null
                    if (!requireVisible()) return@post
                    // done() has already run: get() cannot block on the native call here.
                    val completion = try { get() } catch (_: ExecutionException) { null }
                    surface.acceptResult(request, completion)
                    render()
                }
            }
        }
        work = task
        Thread(task, "EON profile check").start()
    }

    private fun openControls() {
        try {
            if (surface.frame?.expanded == true && surface.consumeAction(eligible())) {
                startAssistantActivity(Intent(context, AssistantActivity::class.java))
            }
        } catch (_: SecurityException) {
            // A role revocation during this call is not a reason to retry.
        } catch (_: ActivityNotFoundException) {
            // The launcher activity may have been disabled in the meantime.
        } finally {
            closeHud()
        }
    }

    private fun clearShowing() {
        discardContext()
        clearMic()
        surface.hide()
        cancelTaskSample()
        clearTone()
        work?.cancel(true)
        work = null
        main.removeCallbacksAndMessages(null)
        if (observingScreenOff) {
            context.applicationContext.unregisterReceiver(screenOff)
            observingScreenOff = false
        }
        render()
    }

    private fun closeHud() {
        surface.retire()
        clearShowing()
        setUiEnabled(false)
        try {
            finish()
        } catch (_: SecurityException) {
            // A revoked binding has no authority to finish a replacement session.
        }
    }

    override fun onHide() {
        clearShowing()
        super.onHide()
    }

    override fun onBackPressed() = closeHud()
    override fun onLockscreenShown() = closeHud()
    override fun onCloseSystemDialogs() = closeHud()

    override fun onDestroy() {
        surface.retire()
        clearShowing()
        hud = null
        super.onDestroy()
    }
}
