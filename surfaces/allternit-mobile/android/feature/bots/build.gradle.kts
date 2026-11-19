// :feature:bots — Bots mode (handoffs/android-native-2026-11-18, wave E). A port of iOS
// Features/{Bots,Agents,AgentActivity,Decisions}: the bots launchpad, bot chat and live stage,
// the bot page and home tabs, threads and tasks, the bot form, agent hub, marketplace,
// workspace files, agent activity, decisions and the bot project page.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.bots"
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
    // Theme, FriendlyState, Haptics and the Compose BOM.
    api(project(":core:design"))
    api(project(":core:api"))
    // The bot chat reuses the chat screen, its view model and backend (relay calls).
    api(project(":feature:chat"))
    // LibraryNet (the /api/v1 routes), the JSON helpers and the library chrome.
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    // rememberLauncherForActivityResult (Hand it something: files and photos).
    implementation(libs.activity.compose)
    testImplementation(libs.junit)
}
