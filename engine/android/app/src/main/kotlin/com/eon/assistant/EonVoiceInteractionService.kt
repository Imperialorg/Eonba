package com.eon.assistant

import android.content.Intent
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession

internal val voiceHud = VoiceHudRegistry()
internal const val NO_ASSIST_CONTEXT =
    VoiceInteractionSession.SHOW_WITH_ASSIST or VoiceInteractionSession.SHOW_WITH_SCREENSHOT

class EonVoiceInteractionService : VoiceInteractionService() {
    private var owner: VoiceHudRegistry.Binding? = null

    override fun onReady() {
        super.onReady()
        release()
        try {
            setDisabledShowContext(NO_ASSIST_CONTEXT)
            if (disabledShowContext and NO_ASSIST_CONTEXT == NO_ASSIST_CONTEXT) {
                owner = VoiceHudRegistry.Binding().also(voiceHud::ready)
                ControlsAssist.ready(this)
            }
        } catch (_: SecurityException) {
            // The system can switch assistants while the service is starting.
        }
    }

    private fun release() {
        ControlsAssist.release(this)
        owner?.let(voiceHud::shutdown)
        owner = null
        InspectionAccess.cancel()
    }

    override fun onShutdown() {
        release()
        super.onShutdown()
    }

    override fun onUnbind(intent: Intent?): Boolean {
        release()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        release()
        super.onDestroy()
    }
}
