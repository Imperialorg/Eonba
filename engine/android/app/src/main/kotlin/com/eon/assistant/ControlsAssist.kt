package com.eon.assistant

import android.app.KeyguardManager
import android.app.role.RoleManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import android.service.voice.VoiceInteractionService
import android.service.voice.VoiceInteractionSession
import java.util.UUID

/** One opt-in, one showing, one callback. Main thread only; no persisted authorization. */
internal class ControlsConsent(private val clock: () -> Long) {
    class Ticket(val source: Any, val key: String, val until: Long) {
        var shown = false
    }

    private var current: Ticket? = null
    fun issue(source: Any) = Ticket(source, UUID.randomUUID().toString(), clock() + 5_000)
        .also { current = it }
    fun live() = current?.takeIf { clock() < it.until }
    fun show(key: String?): Ticket? = live()?.takeIf { !it.shown && it.key == key }
        ?.also { it.shown = true }
    fun consume(ticket: Ticket): Boolean {
        val ok = live() === ticket && ticket.shown
        cancel(ticket)
        return ok
    }
    fun cancel(ticket: Ticket?) { if (current === ticket) current = null }
    fun reset() { current = null }
}

internal fun selectedAndUnlocked(context: Context): Boolean = try {
    val role = context.getSystemService(RoleManager::class.java)
    context.getSystemService(PowerManager::class.java)?.isInteractive == true &&
        context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked == false &&
        role?.isRoleAvailable(RoleManager.ROLE_ASSISTANT) == true && role.isRoleHeld(RoleManager.ROLE_ASSISTANT) &&
        VoiceInteractionService.isActiveService(context, ComponentName(context, EonVoiceInteractionService::class.java))
} catch (_: SecurityException) {
    false
}

/** Only Activity.showAssist scopes the OS request to the calling, top, visible activity token.
 * Never request context for a gesture, another app, or a later invocation. */
internal object ControlsAssist {
    private const val KEY = "com.eon.assistant.CONTROLS_REVIEW"
    private val consent = ControlsConsent(SystemClock::elapsedRealtime)
    private val main = Handler(Looper.getMainLooper())
    private var service: EonVoiceInteractionService? = null
    private var displayed: ControlsConsent.Ticket? = null

    fun ready(owner: EonVoiceInteractionService) {
        consent.reset()
        displayed = null
        service = owner
    }
    fun release(owner: EonVoiceInteractionService) {
        if (service !== owner) return
        consent.reset()
        displayed = null
        service = null
    }

    fun request(activity: AssistantActivity, source: Any): Boolean {
        val owner = service ?: return false
        if (!activity.hasWindowFocus() || activity.isInMultiWindowMode || activity.isInPictureInPictureMode ||
            !selectedAndUnlocked(activity)) return false
        val ticket = consent.issue(source)
        var accepted = false
        try {
            if (owner.disabledShowContext == NO_ASSIST_CONTEXT) {
                // This policy is global to the binding. Keep screenshots disabled, and only open
                // structure delivery across the synchronous, activity-token-scoped Binder call.
                owner.setDisabledShowContext(VoiceInteractionSession.SHOW_WITH_SCREENSHOT)
                accepted = owner.disabledShowContext == VoiceInteractionSession.SHOW_WITH_SCREENSHOT &&
                    activity.showAssist(Bundle().apply { putString(KEY, ticket.key) })
            }
        } catch (_: RuntimeException) {
            accepted = false
        } finally {
            try {
                owner.setDisabledShowContext(NO_ASSIST_CONTEXT)
                accepted = accepted && owner.disabledShowContext == NO_ASSIST_CONTEXT
            } catch (_: RuntimeException) {
                accepted = false
            }
            if (!accepted) consent.cancel(ticket)
        }
        if (accepted) main.postDelayed({ consent.cancel(ticket) }, 5_000)
        return accepted
    }

    fun supply(source: Any, data: Bundle) {
        val ticket = consent.live() ?: return
        val owner = service ?: return
        if (ticket.source === source && selectedAndUnlocked(owner)) data.putString(KEY, ticket.key)
    }

    fun cancelSource(source: Any) {
        consent.cancel(consent.live()?.takeIf { it.source === source })
        if (displayed?.source === source) displayed = null
    }

    fun cancel(ticket: ControlsConsent.Ticket?) {
        consent.cancel(ticket)
        if (displayed === ticket) displayed = null
    }
    fun mayDisplay(ticket: ControlsConsent.Ticket) =
        displayed === ticket && SystemClock.elapsedRealtime() < ticket.until + 25_000

    fun beginShow(args: Bundle?, flags: Int): ControlsConsent.Ticket? {
        if (consent.live() == null) return null // Ordinary invocations never inspect their args.
        val ticket = if ((flags and NO_ASSIST_CONTEXT.inv()) == VoiceInteractionSession.SHOW_SOURCE_APPLICATION &&
            flags and VoiceInteractionSession.SHOW_WITH_ASSIST != 0) try {
            consent.show(args?.getString(KEY))
        } catch (_: RuntimeException) { null } else null
        if (ticket == null) consent.reset()
        return ticket
    }

    fun protectedWindows(context: Context, ticket: ControlsConsent.Ticket,
                         state: VoiceInteractionSession.AssistState): Int? {
        if (!consent.consume(ticket) || state.index != 0 || state.count != 1) return null
        return try {
            val structure = state.assistStructure ?: return null
            if (structure.activityComponent != ComponentName(context, AssistantActivity::class.java)) return null
            val envelope = state.assistData ?: return null
            if (envelope.getInt(Intent.EXTRA_ASSIST_UID, -1) != Process.myUid() ||
                envelope.getBundle(Intent.EXTRA_ASSIST_CONTEXT)?.getString(KEY) != ticket.key) return null
            // Count only secure window roots. No window titles, text, children, URLs or content.
            structure.windowNodeCount.takeIf { count -> count in 1..8 &&
                (0 until count).all { structure.getWindowNodeAt(it).rootViewNode.isAssistBlocked } }
                ?.also { displayed = ticket }
        } catch (_: RuntimeException) { null }
    }
}
