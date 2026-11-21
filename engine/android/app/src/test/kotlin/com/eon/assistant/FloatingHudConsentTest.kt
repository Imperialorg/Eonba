package com.eon.assistant

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class FloatingHudConsentTest {
    private val consent = FloatingHudConsent()

    @Test fun grantsDoNotStartAndPermissionMustStillHoldOnDelivery() {
        assertNull(consent.request(false))
        assertFalse(consent.consume(null, true))
        assertFalse(consent.consume("notification", true))
        val ticket = assertNotNull(consent.request(true))
        assertFalse(consent.accepts(ticket))
        assertFalse(consent.consume(ticket, false))
        assertFalse(consent.consume(ticket, true))
        assertFalse(consent.startedOrPending)
    }

    @Test fun stopWhileStartIsQueuedPreventsALateWindow() {
        val old = assertNotNull(consent.request(true))
        consent.end()
        val current = assertNotNull(consent.request(true))
        assertNotEquals(old, current)
        assertFalse(consent.consume(old, true))
        assertTrue(consent.consume(current, true))
        assertFalse(consent.accepts(old))
        assertTrue(consent.accepts(current))
    }

    @Test fun onlyOneRunAndNeitherRedeliveryNorOldNoticeActionsCanRestartIt() {
        val ticket = assertNotNull(consent.request(true))
        assertNull(consent.request(true))
        assertTrue(consent.consume(ticket, true))
        assertFalse(consent.consume(ticket, true))
        assertNull(consent.request(true))
        consent.end()
        assertFalse(consent.accepts(ticket))
        assertFalse(consent.consume(ticket, true))
        val afterProcessDeath = FloatingHudConsent()
        assertFalse(afterProcessDeath.accepts(ticket))
        assertFalse(afterProcessDeath.consume(ticket, true))
    }
}
