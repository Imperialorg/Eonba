package com.eon.assistant

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class VoiceHudPresentationTest {
    private val binding = VoiceHudRegistry.Binding()
    private val registry = VoiceHudRegistry().also { it.ready(binding) }
    private var revoked = 0
    private val hud = registry.Surface(binding) { revoked++ }

    private fun startCheck(): ProfileCheck.Requested {
        assertTrue(hud.show(userInvocation = true, eligible = true))
        hud.toggleDetails()
        return assertNotNull(hud.requestCheck())
    }

    @Test
    fun openingIsIdleAndOnlyAnExplicitExpandedCheckStartsWork() {
        hud.toggleDetails()
        assertNull(hud.frame)
        assertNull(hud.requestCheck())
        assertTrue(hud.show(true, true))
        assertEquals(VoiceHudFrame(expanded = false, check = ProfileCheck.Idle), hud.frame)
        assertNull(hud.requestCheck())
        hud.toggleDetails()
        val request = assertNotNull(hud.requestCheck())
        assertSame(request, hud.frame?.check)
        assertNull(hud.requestCheck())
        hud.toggleDetails()
        // Collapsing the details does not fabricate completion or start another check.
        assertSame(request, hud.frame?.check)
        hud.acceptResult(request, "p0_mobile_ultralight")
        assertEquals(VoiceHudFrame(false, ProfileCheck.Result("p0_mobile_ultralight")), hud.frame)
    }

    @Test
    fun aFailedCheckIsNotASuggestionAndOldResultsCannotWinARetry() {
        val first = startCheck()
        hud.acceptResult(first, null)
        assertEquals(ProfileCheck.Result(null), hud.frame?.check)
        val retry = assertNotNull(hud.requestCheck())
        hud.acceptResult(first, "old")
        assertSame(retry, hud.frame?.check)
        hud.acceptResult(retry, "new")
        hud.acceptResult(retry, null)
        assertEquals(ProfileCheck.Result("new"), hud.frame?.check)
    }

    @Test
    fun hideAndRepeatedInvocationDropDetailsResultsAndOutstandingRequests() {
        val hidden = startCheck()
        hud.hide()
        hud.acceptResult(hidden, "late")
        assertNull(hud.frame)
        val firstShowing = startCheck()
        val nextShowing = startCheck() // same OS session object
        hud.acceptResult(firstShowing, "late")
        assertSame(nextShowing, hud.frame?.check)
        hud.acceptResult(nextShowing, "finished")
        assertTrue(hud.show(true, true))
        assertEquals(VoiceHudFrame(), hud.frame)
        assertFalse(hud.show(false, true))
        assertNull(hud.frame)
    }

    @Test
    fun revocationClearsTheFrameBeforeTheCloseCallbackAndRejectsLateNativeResults() {
        lateinit var old: VoiceHudRegistry.Surface
        old = registry.Surface(binding) {
            assertNull(old.frame)
            assertNull(old.requestCheck())
            revoked++
        }
        assertTrue(old.show(true, true))
        old.toggleDetails()
        val oldCheck = assertNotNull(old.requestCheck())
        val current = startCheck()
        assertEquals(1, revoked)
        old.acceptResult(oldCheck, "stale")
        assertNull(old.frame)
        assertSame(current, hud.frame?.check)
        registry.shutdown(binding)
        hud.acceptResult(current, "also stale")
        assertNull(hud.frame)
        assertEquals(2, revoked)
    }

    @Test
    fun losingEligibilityOrEndingTheSessionClearsEvenAPendingResult() {
        val denied = startCheck()
        assertFalse(hud.consumeAction(false))
        hud.acceptResult(denied, "late")
        assertNull(hud.frame)
        val ended = startCheck()
        hud.retire()
        hud.acceptResult(ended, "late")
        assertNull(hud.frame)
        assertFalse(hud.show(true, true))
    }
}
