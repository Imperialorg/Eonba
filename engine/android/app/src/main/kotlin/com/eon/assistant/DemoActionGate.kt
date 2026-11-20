package com.eon.assistant

internal enum class DemoAction { CLICK_COUNTER, SCROLL_SAMPLES_FORWARD }

/**
 * Main-thread only. The surface supplies exactly two native controls from its private demo tree.
 * A review identifies an action, not an address: the exact child and live scope must still match.
 */
internal class DemoActionGate<N : Any>(
    private val counter: N,
    private val samples: N,
    private val childAt: (Int) -> N?,
    private val supports: (N, DemoAction) -> Boolean,
    private val scope: () -> Any?,
    private val now: () -> Long,
    private val dispatch: (N, DemoAction) -> Boolean,
) {
    companion object { const val REVIEW_MS = 15_000L }

    class Review internal constructor(val action: DemoAction, internal val scope: Any, internal val started: Long) {
        internal var spent = false
    }

    private var current: Review? = null
    fun cancel() { current = null }

    private fun resolve(action: DemoAction): N? {
        val node = when (action) {
            DemoAction.CLICK_COUNTER -> childAt(0)?.takeIf { it === counter }
            DemoAction.SCROLL_SAMPLES_FORWARD -> childAt(1)?.takeIf { it === samples }
        }
        return node?.takeIf { supports(it, action) }
    }

    fun review(action: DemoAction): Review? {
        cancel()
        val environment = scope() ?: return null
        if (resolve(action) == null || scope() !== environment) return null
        return Review(action, environment, now()).also { current = it }
    }

    fun confirm(review: Review): Boolean {
        if (current !== review || review.spent) return false
        review.spent = true
        fun valid() = scope() === review.scope && current === review && now() - review.started in 0 until REVIEW_MS
        try {
            if (!valid()) return false
            val node = resolve(review.action) ?: return false
            return valid() && dispatch(node, review.action)
        } finally {
            if (current === review) cancel()
        }
    }
}
