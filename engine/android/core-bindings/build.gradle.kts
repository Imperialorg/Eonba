plugins {
    kotlin("jvm") version "2.3.0"
}

dependencies {
    api("net.java.dev.jna:jna:5.14.0")
    testImplementation(kotlin("test"))
}

// The Rust engine is built and its Kotlin bindings are generated from the same cdylib that the tests load, so the
// bindings and the native library always come from one build.
val engineDir = rootProject.projectDir.resolve("../core-rs")
val cargoTargetDir = providers.environmentVariable("CARGO_TARGET_DIR").orElse("/tmp/cargo-target")
val nativeLibDir = cargoTargetDir.map { "$it/debug" }
val generatedKotlin = layout.buildDirectory.dir("generated/uniffi/kotlin")

val buildNativeLibrary by tasks.registering(Exec::class) {
    workingDir = engineDir
    commandLine("cargo", "build", "-q", "-p", "eon-uniffi")
}

val generateBindings by tasks.registering(Exec::class) {
    dependsOn(buildNativeLibrary)
    workingDir = engineDir
    val outDir = generatedKotlin.get().asFile.absolutePath
    commandLine(
        "cargo", "run", "-q", "-p", "eon-uniffi", "--bin", "uniffi-bindgen", "--",
        "generate", "--library", "${nativeLibDir.get()}/libeon_uniffi.so",
        "--language", "kotlin", "--out-dir", outDir, "--no-format",
    )
}

sourceSets {
    main {
        kotlin.srcDir(generatedKotlin)
    }
}

tasks.named("compileKotlin") {
    dependsOn(generateBindings)
}

tasks.test {
    dependsOn(buildNativeLibrary)
    useJUnitPlatform()
    systemProperty("jna.library.path", nativeLibDir.get())
}
