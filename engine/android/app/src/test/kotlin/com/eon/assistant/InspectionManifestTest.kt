package com.eon.assistant

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.w3c.dom.Element
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class InspectionManifestTest {
    private val android = "http://schemas.android.com/apk/res/android"
    private fun root(path: String): Element = DocumentBuilderFactory.newInstance().apply {
        isNamespaceAware = true
        setFeature("http://apache.org/xml/features/disallow-doctype-decl", true)
    }.newDocumentBuilder().parse(File("src/main/$path")).documentElement

    private fun Element.elements(tag: String) = getElementsByTagName(tag).let { nodes ->
        (0 until nodes.length).map { nodes.item(it) as Element }
    }
    private fun Element.attr(name: String) = getAttributeNS(android, name)

    @Test
    fun noCaptureOrOverlayPermissionAndOnlyOneSystemGuardedService() {
        val manifest = root("AndroidManifest.xml")
        assertEquals(listOf("android.permission.HIDE_OVERLAY_WINDOWS"),
            manifest.elements("uses-permission").map { it.attr("name") })
        val service = manifest.elements("service").single()
        assertEquals(".FocusedControlService", service.attr("name"))
        assertEquals("android.permission.BIND_ACCESSIBILITY_SERVICE", service.attr("permission"))
        assertEquals("@bool/focused_control_supported", service.attr("enabled"))
        assertEquals("@xml/focused_control_service", service.elements("meta-data").single().attr("resource"))
        val result = manifest.elements("activity").single { it.attr("name") == ".InspectionResultActivity" }
        assertEquals("false", result.attr("exported"))
        assertEquals("true", result.attr("noHistory"))
        assertEquals("true", result.attr("excludeFromRecents"))
        assertTrue(manifest.elements("receiver").isEmpty())
    }

    @Test
    fun focusServiceOptsOutOfEventsGestureInputAndSensitiveData() {
        val service = root("res/xml/focused_control_service.xml")
        assertEquals("true", service.attr("canRetrieveWindowContent"))
        assertEquals("false", service.attr("isAccessibilityTool"))
        assertEquals("flagRequestAccessibilityButton", service.attr("accessibilityFlags"))
        for (attribute in listOf("accessibilityEventTypes", "canPerformGestures", "canTakeScreenshot",
            "canRequestFilterKeyEvents", "canRequestTouchExplorationMode", "canControlMagnification")) {
            assertFalse(service.hasAttributeNS(android, attribute), attribute)
        }
        for ((dir, expected) in listOf("values" to "false", "values-v34" to "true")) {
            val enabled = root("res/$dir/inspector.xml").elements("bool").single { it.getAttribute("name") == "focused_control_supported" }
            assertEquals(expected, enabled.textContent.trim())
        }
    }
}
