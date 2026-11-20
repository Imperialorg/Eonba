package com.eon.assistant

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DemoActionGateTest {
    private class Node(val offers: DemoAction? = null, vararg children: Node) {
        val children = children.toMutableList()
        var available = true
    }

    private class Fixture {
        val count = Node(DemoAction.CLICK_COUNTER)
        val samples = Node(DemoAction.SCROLL_SAMPLES_FORWARD, Node(), Node())
        val root = Node(null, count, samples)
        var epoch: Any? = Any()
        var time = 700L
        val reads = mutableListOf<Int>()
        val checked = mutableListOf<Node>()
        val actions = mutableListOf<Pair<Node, DemoAction>>()
        var onCheck: () -> Unit = {}
        var onDispatch: () -> Boolean = { true }
        val gate = DemoActionGate(count, samples, { slot ->
            reads.add(slot)
            root.children.getOrNull(slot)
        }, { node, action ->
            checked.add(node)
            onCheck()
            node.available && node.offers == action
        }, { epoch }, { time }) { node, action ->
            actions.add(node to action)
            onDispatch()
        }
        fun click() = assertNotNull(gate.review(DemoAction.CLICK_COUNTER))
        fun scroll() = assertNotNull(gate.review(DemoAction.SCROLL_SAMPLES_FORWARD))
    }

    @Test fun eachConfirmationPerformsOnlyItsExactActionOnce() = with(Fixture()) {
        for ((target, action) in listOf(count to DemoAction.CLICK_COUNTER, samples to DemoAction.SCROLL_SAMPLES_FORWARD)) {
            val before = actions.size
            val r = assertNotNull(gate.review(action))
            assertEquals(before, actions.size)
            assertTrue(gate.confirm(r))
            assertFalse(gate.confirm(r))
            assertEquals(before + 1, actions.size)
            assertEquals(target to action, actions.last())
        }
        assertEquals(listOf(0, 0, 1, 1), reads)
    }

    @Test fun unrelatedSameShapedNodesCannotBeSubstitutedAndAreNotInspected() = with(Fixture()) {
        val r = click()
        root.children[0] = Node(DemoAction.CLICK_COUNTER, count)
        assertFalse(gate.confirm(r))
        assertNull(gate.review(DemoAction.CLICK_COUNTER))
        assertEquals(listOf(count), checked)
        assertTrue(actions.isEmpty())
    }

    @Test fun replacementOrCancellationRetiresTheOldConfirmation() = with(Fixture()) {
        val first = click()
        val second = scroll()
        assertFalse(gate.confirm(first))
        assertTrue(gate.confirm(second))
        val third = click()
        gate.cancel()
        assertFalse(gate.confirm(third))
        assertEquals(listOf(samples to DemoAction.SCROLL_SAMPLES_FORWARD), actions)
    }

    @Test fun foreignSurfaceReviewCannotAct() = with(Fixture()) {
        val other = Fixture()
        assertFalse(other.gate.confirm(click()))
        assertTrue(other.reads.isEmpty())
        assertTrue(other.actions.isEmpty())
    }

    @Test fun expiredReviewCannotInspectOrDispatchAtDeadline() = with(Fixture()) {
        val r = scroll()
        time += DemoActionGate.REVIEW_MS
        assertFalse(gate.confirm(r))
        time--
        assertFalse(gate.confirm(r))
        assertEquals(listOf(1), reads)
        assertTrue(actions.isEmpty())
    }

    @Test fun unavailableOrReconnectedEnvironmentCannotUseEarlierConsent() = with(Fixture()) {
        val r = click()
        epoch = null
        assertFalse(gate.confirm(r))
        assertNull(gate.review(DemoAction.CLICK_COUNTER))
        epoch = Any()
        val second = click()
        epoch = Any() // Same package and Views, but not the same uninterrupted connection.
        assertFalse(gate.confirm(second))
        assertEquals(listOf(0, 0), reads)
        assertTrue(actions.isEmpty())
    }

    @Test fun lossOfActionSupportIsRecheckedAndConsumesConsent() = with(Fixture()) {
        val r = scroll()
        samples.available = false
        assertFalse(gate.confirm(r))
        samples.available = true
        assertFalse(gate.confirm(r))
        assertTrue(actions.isEmpty())
    }

    @Test fun revocationDuringTargetResolutionPreventsDispatch() = with(Fixture()) {
        val r = click()
        onCheck = gate::cancel
        assertFalse(gate.confirm(r))
        assertTrue(actions.isEmpty())
    }

    @Test fun reentryAndProviderFailureDoNotReopenApproval() = with(Fixture()) {
        val r = click()
        onDispatch = {
            assertFalse(gate.confirm(r))
            error("detached")
        }
        assertFailsWith<IllegalStateException> { gate.confirm(r) }
        assertFalse(gate.confirm(r))
        assertEquals(1, actions.size)
    }
}
