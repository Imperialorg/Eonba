# ANDROID INTEGRATION SPECIFICATION
### Mobile Sensory Presence & Accessibility Bridge

> **Subsystem:** EON Mobile Client  
> **Directory:** `clients/android/`  
> **Platform:** Android 14+ (API 34/35), Min SDK 28  
> **Authority:** CANONICAL BLUEPRINT FOR ANDROID IMPLEMENTATION

---

## 1. Overview & Architectural Role

The Android application (`com.eon.assistant`) serves as EON's mobile eyes, ears, and hands. It connects to the Native Rust Core (`libeon_uniffi.so`) via Mozilla UniFFI.

The Android subsystem provides:
1. **Accessibility Automation**: `EonAccessibilityService` traverses the user-authorized UI tree, finds clickable elements, dispatches gestures, and monitors window state transitions.
2. **Persistent HUD & Thought Visualizer**: `EonOverlayService` renders a non-intrusive floating orb/card over other apps (`TYPE_APPLICATION_OVERLAY`).
3. **Audio Capture Loop**: Feeds microphone PCM buffers directly to native Rust VAD/ASR via shared zero-copy buffers.
4. **Quick Settings Tile**: `EonTileService` allows one-tap toggling of EON between Dormant, Silent, and Full-Duplex Voice modes.

---

## 2. Android Manifest & Permissions (`AndroidManifest.xml`)

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="com.eon.assistant">

    <!-- Audio & Speech -->
    <uses-permission android:name="android.permission.RECORD_AUDIO" />
    <uses-permission android:name="android.permission.MODIFY_AUDIO_SETTINGS" />

    <!-- Overlay & System Interaction -->
    <uses-permission android:name="android.permission.SYSTEM_ALERT_WINDOW" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_MICROPHONE" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE" />
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />

    <!-- Network & P2P Mesh -->
    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    <uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
    <uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />
    <uses-permission android:name="android.permission.BLUETOOTH" />
    <uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />

    <!-- Hardware Sensory (Optional) -->
    <uses-permission android:name="android.permission.CAMERA" />
    <uses-permission android:name="android.permission.VIBRATE" />

    <application
        android:name=".EonApplication"
        android:allowBackup="false"
        android:icon="@mipmap/ic_launcher"
        android:label="EON"
        android:roundIcon="@mipmap/ic_launcher_round"
        android:supportsRtl="true"
        android:theme="@style/Theme.Eon">

        <!-- Persistent Core Foreground Service -->
        <service
            android:name=".service.EonCoreService"
            android:exported="false"
            android:foregroundServiceType="microphone|connectedDevice" />

        <!-- Native Accessibility Controller -->
        <service
            android:name=".accessibility.EonAccessibilityService"
            android:permission="android.permission.BIND_ACCESSIBILITY_SERVICE"
            android:exported="true">
            <intent-filter>
                <action android:name="android.accessibilityservice.AccessibilityService" />
            </intent-filter>
            <meta-data
                android:name="android.accessibilityservice"
                android:resource="@xml/accessibility_service_config" />
        </service>

        <!-- Floating HUD Overlay Service -->
        <service
            android:name=".overlay.EonOverlayService"
            android:exported="false" />

        <!-- Quick Settings Tile -->
        <service
            android:name=".system.EonTileService"
            android:icon="@drawable/ic_eon_tile"
            android:label="EON"
            android:permission="android.permission.BIND_QUICK_SETTINGS_TILE"
            android:exported="true">
            <intent-filter>
                <action android:name="android.service.quicksettings.action.QS_TILE" />
            </intent-filter>
        </service>

    </application>
</manifest>
```

---

## 3. Accessibility Service Configuration (`res/xml/accessibility_service_config.xml`)

```xml
<?xml version="1.0" encoding="utf-8"?>
<accessibility-service xmlns:android="http://schemas.android.com/apk/res/android"
    android:accessibilityEventTypes="typeWindowStateChanged|typeWindowContentChanged|typeViewClicked"
    android:accessibilityFeedbackType="feedbackGeneric"
    android:accessibilityFlags="flagDefault|flagRetrieveInteractiveWindows|flagReportViewIds|flagIncludeNotImportantViews"
    android:canRetrieveWindowContent="true"
    android:canPerformGestures="true"
    android:notificationTimeout="50"
    android:description="@string/accessibility_service_description" />
```

---

## 4. UniFFI Kotlin Integration Pattern

In `EonCoreService.kt`, the Android service initializes the native Rust engine and sets up the streaming audio pipeline:

```kotlin
package com.eon.assistant.service

import android.app.Service
import android.content.Intent
import android.os.IBinder
import uniffi.eon_core.*

class EonCoreService : Service() {
    private lateinit var kernelBridge: EonKernelBridge
    private lateinit var voiceBridge: EonVoiceBridge

    override fun onCreate() {
        super.onCreate()
        val storagePath = applicationContext.filesDir.absolutePath
        val identityId = "id_01J9H2X7K9M4N3P2R1Q8S7T6V5"

        // Initialize Rust Native Core
        kernelBridge = EonKernelBridge(storagePath, identityId)
        voiceBridge = EonVoiceBridge()

        // Register hardware barge-in callback
        voiceBridge.registerBargeInCallback(object : BargeInCallback {
            override fun onBargeInDetected(timestampMs: Long) {
                // Instantly mute speaker and show listening state on overlay
                EonOverlayService.showListeningState(applicationContext)
            }
        })
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
```

