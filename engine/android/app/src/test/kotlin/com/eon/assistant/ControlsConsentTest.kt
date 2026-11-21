package com.eon.assistant

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class ControlsConsentTest {
    private var now = 0L
    private val gate = ControlsConsent { now }

    @Test fun onlyTheSameFreshRequestCanShowThenReadOnce() {
        val source = Any()
        val old = gate.issue(source)
        val fresh = gate.issue(source)
        assertNotEquals(old.key, fresh.key)
        assertNull(gate.show(old.key))
        gate.cancel(old)
        assertSame(fresh, gate.show(fresh.key))
        assertNull(gate.show(fresh.key))
        assertFalse(gate.consume(old))
        assertTrue(gate.consume(fresh))
        assertFalse(gate.consume(fresh))
        assertNull(gate.show(fresh.key))
    }

    @Test fun callbacksWithoutAShowOrAfterExpiryRevocationOrRestartCannotRead() {
        assertFalse(gate.consume(gate.issue(Any())))
        val ticket = gate.issue(Any())
        assertSame(ticket, gate.show(ticket.key))
        now = ticket.until
        assertFalse(gate.consume(ticket))
        val next = gate.issue(Any())
        gate.reset()
        assertNull(gate.show(next.key))
        val last = gate.issue(Any())
        assertSame(last, gate.show(last.key))
        val restarted = ControlsConsent { now }
        assertNull(restarted.show(last.key))
        assertFalse(restarted.consume(last))
    }
}
