// :feature:settings: Settings part 1 (wave G1). A port of iOS Features/Settings: the Settings root, Advanced, and the
// Account, Privacy, Notifications, Memory, Device, Models, Subagents, MCP and Webhooks pages.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.settings"
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
    // Gap run GS-B: the Files key ring (approve, decline, rotate), Keystore boxes and the voice key hand-off crypto.
    implementation(project(":core:files"))
    // The voice key QR scan: CameraX preview + ML Kit barcode scanning (bundled model, no Play services download).
    implementation(libs.camerax.camera2)
    implementation(libs.camerax.lifecycle)
    implementation(libs.camerax.view)
    implementation(libs.mlkit.barcode)
    api(project(":core:design"))
    api(project(":core:api"))
    // ChatBackend (the relay to the user's computer: MCP servers).
    api(project(":feature:chat"))
    // LibraryNet (the /api/v1 routes) and the JSON helpers.
    implementation(project(":feature:artifacts"))
    // "Who can use this" on a connected connector (Team map, flag `team.map`).
    implementation(project(":feature:bots"))
    implementation(project(":core:auth"))
    implementation(libs.activity.compose)
    // Wave H1: Billing (Google Play Billing, the store pieces, the account plan state).
    implementation(project(":feature:billing"))
    // Wave I1: Settings > Notifications registers this phone for push after the permission.
    implementation(project(":feature:push"))
    // In-app Custom Tab for links (iOS SafariView).
    implementation(libs.androidx.browser)
    // Response style > Try this style reads the chat event stream (SseParser, AgentChatFrames).
    implementation(project(":core:stream"))
    testImplementation(libs.junit)
}
