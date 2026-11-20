// :feature:chat — the native chat screen (handoffs/android-native-2026-11-18, wave A).
// ChatScreen + ChatViewModel, a port of iOS Features/Chat: send and stream a turn, history,
// abort, edit-resend, the message cards, permission asks and the no-computer state.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.chat"
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
    // Theme, FriendlyState, ThinkingOrb, Haptics and the Compose BOM.
    api(project(":core:design"))
    // The generated client (sessions, messages) and ApiCall for the hand-written routes.
    api(project(":core:api"))
    // POST /api/agent-chat frames (AgentChatStream, ChatStreamEvent).
    implementation(project(":core:stream"))
    // AllternitAuth.tokenProvider for the instance and upload calls.
    implementation(project(":core:auth"))
    // rememberLauncherForActivityResult (save a run_code file where the person picks).
    implementation(libs.activity.compose)
    // Wave H1: Cloud credits and the store sheet (Google Play Billing) for the credits and compute gates.
    implementation(project(":feature:billing"))
    // Gap run GS-D: chat turns on the phone (ondevice/<id>#<quant> brains).
    implementation(project(":ondevice"))
    // Gap run GS-D: the own-key voice engine reads the Keystore-sealed voice keys (SealedSecret).
    implementation(project(":core:files"))
    testImplementation(libs.junit)
}
