package com.eon.assistant

/** Main-thread only. A window belongs to one OS binding; neither survives a process restart. */
internal class VoiceHudRegistry {
    class Binding
    var binding: Binding? = null
        private set
    private var visible: Surface? = null

    fun ready(newBinding: Binding) {
        binding = newBinding
        revokeVisible()
    }

    fun shutdown(oldBinding: Binding) {
        if (binding !== oldBinding) return
        binding = null
        revokeVisible()
    }

    private fun revokeVisible() {
        val old = visible
        old?.retire()
        old?.onRevoked?.invoke()
    }

    inner class Surface(private val owner: Binding?, internal val onRevoked: () -> Unit) {
        private var retired = false

        fun show(userInvocation: Boolean, eligible: Boolean): Boolean {
            if (retired || owner == null || owner !== binding || !userInvocation || !eligible) {
                hide()
                return false
            }
            if (visible !== this) revokeVisible()
            visible = this
            return true
        }

        fun canUse(eligible: Boolean): Boolean = visible === this && owner === binding && eligible

        fun consumeAction(eligible: Boolean): Boolean {
            val allowed = canUse(eligible)
            hide()
            return allowed
        }

        fun hide() {
            if (visible === this) visible = null
        }

        fun retire() {
            retired = true
            hide()
        }
    }
}
