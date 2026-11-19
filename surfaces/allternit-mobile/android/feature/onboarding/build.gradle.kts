// :feature:onboarding — native sign-in (handoffs/android-native-2026-11-18, step 7b).
// The twin of iOS LoginGateView (ios/App/AllternitApp.swift). Only :app's nativeapp
// flavor reaches this module (through :feature:shell).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.onboarding"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    api(project(":core:design"))
    api(project(":core:auth"))
}

// The sign-in loop (LoginArrival.kt) plays the same offline page as iOS and the web:
// shared/login-arrival/login-arrival.html, built by shared/login-arrival/build.py. It is copied
// into this module's generated assets so there is one committed copy, never a second one here.
abstract class SyncLoginArrival : DefaultTask() {
    @get:InputFile
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val html: RegularFileProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun copy() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        html.get().asFile.copyTo(out.resolve("login-arrival.html"))
    }
}

val syncLoginArrival = tasks.register<SyncLoginArrival>("syncLoginArrival") {
    html.set(rootProject.layout.projectDirectory.file("../shared/login-arrival/login-arrival.html"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.assets?.addGeneratedSourceDirectory(syncLoginArrival, SyncLoginArrival::outputDir)
    }
}
