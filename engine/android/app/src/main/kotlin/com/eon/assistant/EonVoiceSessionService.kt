package com.eon.assistant

import android.app.KeyguardManager
import android.app.role.RoleManager
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Bundle
import android.os.PowerManager
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import android.service.voice.VoiceInteractionSessionService
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

class EonVoiceSessionService : VoiceInteractionSessionService() {
    override fun onNewSession(args: Bundle?): VoiceInteractionSession = EonVoiceSession(this)
}

internal fun userInvokedVoiceHud(flags: Int): Boolean = when (flags and NO_ASSIST_CONTEXT.inv()) {
    VoiceInteractionSession.SHOW_SOURCE_ASSIST_GESTURE, VoiceInteractionSession.SHOW_SOURCE_PUSH_TO_TALK -> true
    else -> false
}

private class EonVoiceSession(context: Context) : VoiceInteractionSession(context) {
    private val surface = voiceHud.Surface(voiceHud.binding, ::closeHud)

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
        setUiEnabled(surface.show(userInvokedVoiceHud(showFlags), eligible()))
    }

    override fun onShow(args: Bundle?, showFlags: Int) {
        if (!surface.canUse(eligible())) {
            closeHud()
            return
        }
        InspectionAccess.cancel()
        window?.window?.apply {
            addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            setHideOverlayWindows(true)
        }
    }

    // Framework callbacks can race a policy or role change. Do not unpack any payload.
    override fun onHandleAssist(state: AssistState) = Unit
    override fun onHandleScreenshot(screenshot: Bitmap?) = Unit

    override fun onCreateContentView(): View = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        setBackgroundColor(context.getColor(android.R.color.white))
        val pad = (16 * resources.displayMetrics.density).toInt()
        setPadding(pad, pad, pad, pad)
        addView(TextView(context).apply {
            setTextColor(context.getColor(android.R.color.black))
            setText(R.string.voice_session_status)
            isAccessibilityHeading = true
        })
        addView(Button(context).apply {
            setText(R.string.open_eon_controls)
            filterTouchesWhenObscured = true
            setOnClickListener {
                try {
                    if (surface.consumeAction(eligible())) {
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
        })
        addView(Button(context).apply {
            setText(R.string.close_voice_session)
            setOnClickListener { closeHud() }
        })
    }

    private fun closeHud() {
        surface.retire()
        setUiEnabled(false)
        try {
            finish()
        } catch (_: SecurityException) {
            // A revoked binding has no authority to finish a replacement session.
        }
    }

    override fun onHide() {
        surface.hide()
        super.onHide()
    }

    override fun onBackPressed() = closeHud()
    override fun onLockscreenShown() = closeHud()
    override fun onCloseSystemDialogs() = closeHud()

    override fun onDestroy() {
        surface.retire()
        super.onDestroy()
    }
}
