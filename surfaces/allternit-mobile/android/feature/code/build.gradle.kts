// :feature:code — Code mode (handoffs/android-native-2026-11-18, wave F1). A port of iOS Features/Code:
// the Code home and sessions overview, computer pairing, the changeset review and session diffs,
// the file browser and editor, dev-server ports and preview, and the terminal (a native VT screen
// over the runtime's /terminal/* service and its SSE stream, like iOS SwiftTerm; no WebView or
// xterm.js). A coding agent in a shell gets a native composer (AgentFrame.kt).
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.code"
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

    // AgentFrameTest reads the agent-frame screen corpus shared with the web and iOS parsers
    // (surfaces/allternit-mobile/agent-frame-fixtures) straight from that folder, never a copy.
    sourceSets.getByName("test").resources.srcDir("../../../agent-frame-fixtures")
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    // Theme, FriendlyState, Haptics and the Compose BOM.
    api(project(":core:design"))
    api(project(":core:api"))
    // The bot chat reuses the chat screen, its view model and backend (relay calls).
    api(project(":feature:chat"))
    implementation(project(":core:stream"))
    implementation(libs.androidx.webkit)
    // LibraryNet (the /api/v1 routes), the JSON helpers and the library chrome.
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    // rememberLauncherForActivityResult (Hand it something: files and photos).
    implementation(libs.activity.compose)
    testImplementation(libs.junit)
}
