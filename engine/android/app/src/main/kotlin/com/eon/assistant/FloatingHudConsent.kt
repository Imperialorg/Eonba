package com.eon.assistant

import java.util.UUID

/** Main-thread, process-local. OS grants are prerequisites, never start commands. */
internal class FloatingHudConsent {
    private var pending: String? = null
    var active: String? = null
        private set
    val startedOrPending get() = pending != null || active != null

    fun request(eligible: Boolean): String? {
        if (!eligible || startedOrPending) return null
        return UUID.randomUUID().toString().also { pending = it }
    }

    fun consume(ticket: String?, eligible: Boolean): Boolean {
        if (ticket == null || ticket != pending) return false
        pending = null
        if (!eligible) return false
        active = ticket
        return true
    }

    fun accepts(ticket: String?) = ticket != null && active == ticket

    fun end() {
        pending = null
        active = null
    }
}
