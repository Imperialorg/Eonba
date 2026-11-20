package com.eon.assistant

import android.app.ActivityManager
import android.app.KeyguardManager
import android.app.role.RoleManager
import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import android.view.View
import android.view.WindowManager
import java.util.concurrent.Callable
import java.util.concurrent.ExecutionException
import java.util.concurrent.FutureTask
import uniffi.eon_uniffi.selectModelProfile

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
    private var work: FutureTask<String>? = null
    private var toneStop: CancellationSignal? = null
    private var observingScreenOff = false
    private val screenOff = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_SCREEN_OFF) closeHud()
        }
    }

    private fun eligible(): Boolean {
        return try {
            val role = context.getSystemService(RoleManager::class.java)
            context.getSystemService(PowerManager::class.java)?.isInteractive == true &&
                context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked == false &&
                role?.isRoleAvailable(RoleManager.ROLE_ASSISTANT) == true &&
                role.isRoleHeld(RoleManager.ROLE_ASSISTANT) &&
                VoiceInteractionService.isActiveService(context, ComponentName(context, EonVoiceInteractionService::class.java)) &&
                disabledShowContext and NO_ASSIST_CONTEXT == NO_ASSIST_CONTEXT
        } catch (_: SecurityException) {
            false
        }
    }

    override fun onPrepareShow(args: Bundle?, showFlags: Int) {
        clearShowing()
        setUiEnabled(surface.show(userInvokedVoiceHud(showFlags), eligible()))
    }

    override fun onShow(args: Bundle?, showFlags: Int) {
        if (!requireVisible()) return
        InspectionAccess.cancel()
        try {
            // SCREEN_OFF is a protected system broadcast. Observe only while this card is shown.
            context.applicationContext.registerReceiver(screenOff, IntentFilter(Intent.ACTION_SCREEN_OFF))
            observingScreenOff = true
        } catch (_: SecurityException) {
            closeHud()
            return
        }
        if (requireVisible()) render()
    }

    // Framework callbacks can race a policy or role change. Do not unpack any payload.
    override fun onHandleAssist(state: AssistState) = Unit
    override fun onHandleScreenshot(screenshot: Bitmap?) = Unit

    override fun onCreateContentView(): View {
        window?.window?.apply {
            addFlags(WindowManager.LayoutParams.FLAG_SECURE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL)
            clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
            setBackgroundDrawableResource(android.R.color.transparent)
            setHideOverlayWindows(true)
        }
        return VoiceHudView(context, ::toggleDetails, ::checkProfile, ::analyzeTone, ::clearTone,
            ::openControls, ::closeHud).also {
            hud = it
            it.render(surface.frame)
        }
    }

    override fun onComputeInsets(outInsets: Insets) {
        // Leave the app underneath at its original size; only the floating card takes touches.
        outInsets.contentInsets.set(0, hud?.rootView?.height ?: 0, 0, 0)
        outInsets.touchableInsets = Insets.TOUCHABLE_INSETS_REGION
        outInsets.touchableRegion.setEmpty()
        if (surface.frame != null) hud?.fillTouchableRegion(outInsets.touchableRegion)
    }

    private fun render() = hud?.render(surface.frame)

    private fun requireVisible(): Boolean {
        if (surface.canUse(eligible())) return true
        closeHud()
        return false
    }

    private fun toggleDetails() {
        if (!requireVisible()) return
        if (surface.frame?.expanded == true) clearTone()
        surface.toggleDetails()
        render()
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
        val task = object : FutureTask<String>(Callable {
            val memory = ActivityManager.MemoryInfo()
            app.getSystemService(ActivityManager::class.java).getMemoryInfo(memory)
            selectModelProfile(memory.totalMem / 1_000_000_000.0)
        }) {
            override fun done() {
                if (isCancelled) return
                main.post {
                    if (surface.frame?.check !== request) return@post
                    work = null
                    if (!requireVisible()) return@post
                    // done() has already run: get() cannot block on the native call here.
                    val profile = try { get() } catch (_: ExecutionException) { null }
                    surface.acceptResult(request, profile)
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
        surface.hide()
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
