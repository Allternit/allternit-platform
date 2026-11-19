// :feature:computer — ACI mode (handoffs/android-native-2026-11-18, wave F3). A port of iOS Features/ACI:
// the ACI tab (browser landing, target picker, nudges), the in-app browser, a live agent run, and
// ACI › Computers (pair, view a screen, phones, backups, workflows). The screen viewer is :core:rfb,
// shared with a bot desktop. :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.computer"
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
    api(project(":core:api"))
    // ChatBackend: the relay to the computer's allternit-api (/api/aci/run, stream, stop, approve; /v1/browser-skills).
    api(project(":feature:chat"))
    // The RFB viewer and the computer drive screen (shared with a bot's desktop).
    api(project(":core:rfb"))
    // LibraryNet (the /api/v1 routes), the JSON helpers and the library chrome.
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    implementation(libs.okhttp)
    // BackHandler (the browser and a run go back like the system Back on iOS swipe).
    implementation(libs.activity.compose)
    implementation(libs.kotlinx.serialization.json)
    // Scan to PDF on a computer's Files screens (iOS FilesDocumentScanner uses VisionKit).
    implementation(libs.mlkit.document.scanner)
    testImplementation(libs.junit)
}
