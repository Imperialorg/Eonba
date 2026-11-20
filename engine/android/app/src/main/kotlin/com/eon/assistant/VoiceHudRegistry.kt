package com.eon.assistant

internal sealed interface ProfileCheck {
    data object Idle : ProfileCheck
    // Identity ties a result to one request in one showing of the HUD.
    class Requested : ProfileCheck
    data class Result(val recommendation: String?) : ProfileCheck
}

internal data class VoiceHudFrame(val expanded: Boolean = false, val check: ProfileCheck = ProfileCheck.Idle)

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
        var frame: VoiceHudFrame? = null
            private set

        fun show(userInvocation: Boolean, eligible: Boolean): Boolean {
            if (retired || owner == null || owner !== binding || !userInvocation || !eligible) {
                hide()
                return false
            }
            if (visible !== this) revokeVisible()
            visible = this
            frame = VoiceHudFrame()
            return true
        }

        fun canUse(eligible: Boolean): Boolean = visible === this && owner === binding && eligible

        fun toggleDetails() {
            frame = frame?.let { it.copy(expanded = !it.expanded) }
        }

        fun requestCheck(): ProfileCheck.Requested? {
            val current = frame ?: return null
            if (!current.expanded || current.check is ProfileCheck.Requested) return null
            return ProfileCheck.Requested().also { frame = current.copy(check = it) }
        }

        fun acceptResult(request: ProfileCheck.Requested, recommendation: String?) {
            val current = frame ?: return
            if (current.check === request) {
                frame = current.copy(check = ProfileCheck.Result(recommendation))
            }
        }

        fun consumeAction(eligible: Boolean): Boolean {
            val allowed = canUse(eligible)
            hide()
            return allowed
        }

        fun hide() {
            if (visible === this) visible = null
            frame = null
        }

        fun retire() {
            retired = true
            hide()
        }
    }
}
