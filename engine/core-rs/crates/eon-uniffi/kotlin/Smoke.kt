import uniffi.eon_uniffi.*

fun check(cond: Boolean, msg: String) {
    if (!cond) throw AssertionError(msg)
}

fun main() {
    check(selectModelProfile(16.0) == "p3_workstation_heavy", "16 GB must select P3, got ${selectModelProfile(16.0)}")
    check(selectModelProfile(4.0) == "p0_mobile_ultralight", "4 GB must select P0, got ${selectModelProfile(4.0)}")
    check(endpointSilenceMs(Completeness.COMPLETE) == 300u, "complete tier must be 300 ms")
    check(endpointSilenceMs(Completeness.AMBIGUOUS) == 800u, "ambiguous tier must be 800 ms")
    check(endpointSilenceMs(Completeness.INCOMPLETE) == 1150u, "incomplete tier must be 1150 ms")
    val refused = try {
        selectModelProfile(Double.NaN)
        false
    } catch (e: BridgeException.InvalidInput) {
        e.message.contains("finite")
    }
    check(refused, "NaN memory must raise BridgeException.InvalidInput")
    println("eon-uniffi kotlin smoke: OK")
}
