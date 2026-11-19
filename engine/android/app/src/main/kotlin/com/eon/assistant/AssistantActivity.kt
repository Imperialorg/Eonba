package com.eon.assistant

import android.app.Activity
import android.app.ActivityManager
import android.app.AlertDialog
import android.app.role.RoleManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import uniffi.eon_uniffi.BridgeException
import uniffi.eon_uniffi.selectModelProfile

/** ACTION_ASSIST launches a normal, foreground activity. It doesn't inspect the incoming intent. */
class AssistantActivity : Activity() {
    private lateinit var roleStatus: TextView
    private lateinit var requestRole: Button
    private lateinit var inspectionStatus: TextView
    private var consent: AlertDialog? = null
    private var resumed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        if (Build.VERSION.SDK_INT >= 31) window.setHideOverlayWindows(true)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val pad = (16 * resources.displayMetrics.density).toInt()
            setPadding(pad, pad, pad, pad)
        }
        val result = TextView(this)
        column.addView(TextView(this).apply {
            text = "EON local assistant\nScreen and microphone capture are off. Choosing EON as assistant does not allow inspection."
        })
        column.addView(Button(this).apply {
            text = "Check model profile"
            setOnClickListener {
                val info = ActivityManager.MemoryInfo()
                getSystemService(ActivityManager::class.java).getMemoryInfo(info)
                result.text = try {
                    "Model profile: ${selectModelProfile(info.totalMem / 1_000_000_000.0)}"
                } catch (_: BridgeException) {
                    "Could not select a profile for this device."
                } catch (_: LinkageError) {
                    "The native bridge could not be loaded."
                }
            }
        })
        column.addView(result)
        roleStatus = TextView(this)
        requestRole = Button(this).apply {
            text = "Choose EON as assistant"
            setOnClickListener {
                if (Build.VERSION.SDK_INT >= 29) {
                    val role = getSystemService(RoleManager::class.java)
                    if (role.isRoleAvailable(RoleManager.ROLE_ASSISTANT) && !role.isRoleHeld(RoleManager.ROLE_ASSISTANT)) {
                        try {
                            startActivityForResult(role.createRequestRoleIntent(RoleManager.ROLE_ASSISTANT), 1)
                        } catch (_: ActivityNotFoundException) {
                            roleStatus.text = "The system assistant picker is unavailable."
                        } catch (_: SecurityException) {
                            roleStatus.text = "Assistant selection is restricted on this device."
                        }
                    }
                }
            }
        }
        column.addView(roleStatus)
        column.addView(requestRole)
        column.addView(Button(this).apply {
            text = "Manage default apps in Settings"
            setOnClickListener { openSettings(Settings.ACTION_MANAGE_DEFAULT_APPS_SETTINGS) }
        })
        inspectionStatus = TextView(this)
        column.addView(inspectionStatus)
        if (Build.VERSION.SDK_INT >= 34) {
            column.addView(TextView(this).apply {
                text = "Optional: in Android Accessibility settings, enable EON focused control and assign its accessibility shortcut. Enabling it alone does not read controls. To revoke access, turn the service off there."
            })
            column.addView(Button(this).apply {
                text = "Open Accessibility settings"
                setOnClickListener { openSettings(Settings.ACTION_ACCESSIBILITY_SETTINGS) }
            })
            column.addView(Button(this).apply {
                text = "Review one-time inspection"
                filterTouchesWhenObscured = true
                setOnClickListener { reviewInspection() }
            })
        }
        setContentView(ScrollView(this).apply {
            fitsSystemWindows = true
            addView(column, ViewGroup.LayoutParams(-1, -2))
        })
    }

    private fun openSettings(action: String) {
        try {
            startActivity(Intent(action))
        } catch (_: ActivityNotFoundException) {
            roleStatus.text = "This Settings page is unavailable."
        } catch (_: SecurityException) {
            roleStatus.text = "Settings access is restricted on this device."
        }
    }

    private fun reviewInspection() {
        if (!InspectionAccess.eligible(this)) {
            inspectionStatus.text = "Inspection is off. Unlock the device, select EON as assistant, and enable its accessibility service and shortcut first."
            return
        }
        val dialog = AlertDialog.Builder(this)
            .setTitle("Allow one focused control inspection?")
            .setMessage("EON will close. Focus the control you choose with accessibility, then tap EON’s accessibility shortcut within 30 seconds. EON shows only its kind (such as Button), not its label, contents or value. Hidden, sensitive and editable controls are skipped. Reopen EON to cancel.")
            .setNegativeButton("Cancel", null)
            .setPositiveButton("Allow next shortcut") { _, _ ->
                if (resumed && InspectionAccess.session.allowNextShortcut(InspectionAccess.eligible(this))) {
                    InspectionAccess.clearAfterTimeout()
                    finish()
                } else {
                    inspectionStatus.text = "Inspection was not allowed."
                }
            }
            .create()
        consent = dialog
        dialog.window?.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        if (Build.VERSION.SDK_INT >= 31) dialog.window?.setHideOverlayWindows(true)
        dialog.show()
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).filterTouchesWhenObscured = true
    }

    override fun onResume() {
        super.onResume()
        resumed = true
        // Returning here cancels any outstanding shortcut, including after Settings or recreation.
        InspectionAccess.cancel()
        inspectionStatus.text = if (Build.VERSION.SDK_INT >= 34) {
            "Inspection is off. No context is being read."
        } else {
            "Inspection is unavailable. Android 14 or newer is required to exclude sensitive controls."
        }
        if (Build.VERSION.SDK_INT < 29) {
            roleStatus.text = "Assistant selection is managed in system settings."
            requestRole.isEnabled = false
            return
        }
        val role = getSystemService(RoleManager::class.java)
        val available = role.isRoleAvailable(RoleManager.ROLE_ASSISTANT)
        val held = available && role.isRoleHeld(RoleManager.ROLE_ASSISTANT)
        roleStatus.text = when {
            held -> "EON is the selected assistant."
            available -> "EON is not the selected assistant."
            else -> "This device does not offer an assistant role."
        }
        requestRole.isEnabled = available && !held
    }

    override fun onPause() {
        resumed = false
        consent?.dismiss()
        consent = null
        super.onPause()
    }
}
