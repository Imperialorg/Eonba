import uniffi.eon_uniffi.BridgeException
import uniffi.eon_uniffi.Completeness
import uniffi.eon_uniffi.endpointSilenceMs
import uniffi.eon_uniffi.selectModelProfile
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class EonBindingsTest {
    @Test
    fun deviceMemorySelectsTheMatchingModelProfile() {
        assertEquals("p3_workstation_heavy", selectModelProfile(16.0))
        assertEquals("p0_mobile_ultralight", selectModelProfile(4.0))
    }

    @Test
    fun endpointTiersMatchTheSpecThresholds() {
        assertEquals(300u, endpointSilenceMs(Completeness.COMPLETE))
        assertEquals(800u, endpointSilenceMs(Completeness.AMBIGUOUS))
        assertEquals(1150u, endpointSilenceMs(Completeness.INCOMPLETE))
    }

    @Test
    fun invalidMemoryIsRaisedAsATypedException() {
        val error = assertFailsWith<BridgeException.InvalidInput> { selectModelProfile(Double.NaN) }
        assertTrue(error.message.contains("finite"), error.message)
    }
}
