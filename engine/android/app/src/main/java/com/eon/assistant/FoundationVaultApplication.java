package com.eon.assistant;

import android.app.Application;

/** Load through ART so the packaged Rust broker can register its host VM before native work. */
public final class FoundationVaultApplication extends Application {
    // Providers and instrumentation can start before Application.onCreate().
    static {
        System.loadLibrary("eon_uniffi");
    }
}
