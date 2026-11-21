package com.eon.assistant

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertSame

class ControlsConsentTest {
    private var now = 0L
    private val gate = ControlsConsent { now }

    @Test fun onlyTheSameFreshCapabilityAndReviewedValueCanBeConsumedOnce() {
        val source = Any()
        val old = gate.issue(source, Any(), true)
        val fresh = gate.issue(source, Any(), false)
        gate.cancel(old)
        assertSame(fresh, gate.take(fresh.token, 0))
        assertNull(gate.take(fresh.token, 0))
        val wrongValue = gate.issue(source, Any(), true)
        assertNull(gate.take(wrongValue.token, 0))
        assertNull(gate.take(wrongValue.token, 1))
        val wrongToken = gate.issue(source, Any(), false)
        assertNull(gate.take(Any(), 0))
        assertNull(gate.take(wrongToken.token, 0))
        val timed = gate.issue(source, Any(), true)
        assertSame(timed, gate.take(timed.token, 1))
    }

    @Test fun expiryRevocationAndRestartAllForgetTheCapability() {
        val ticket = gate.issue(Any(), Any(), false)
        now = ticket.until
        assertNull(gate.take(ticket.token, 0))
        val next = gate.issue(Any(), Any(), false)
        gate.reset()
        assertNull(gate.take(next.token, 0))
        val last = gate.issue(Any(), Any(), true)
        assertNull(ControlsConsent { now }.take(last.token, 1))
    }

    @Test fun noProductionPathOpensTheBindingWideCollectionPolicy() {
        val files = File("src/main").walk().filter { it.extension in listOf("java", "kt") }.toList()
        val writes = Regex("""setDisabledShowContext\s*\(\s*([^)]*?)\s*\)""")
        val calls = files.flatMap { f -> writes.findAll(f.readText()).map { f.name to it.groupValues[1] } }
        assertEquals(listOf("EonVoiceInteractionService.kt" to "NO_ASSIST_CONTEXT"), calls)
        assertEquals(3, NO_ASSIST_CONTEXT)
        for (f in files) {
            assertFalse(Regex("""\bdisabledShowContext\s*=(?!=)""").containsMatchIn(f.readText()), f.name)
        }
    }
}
