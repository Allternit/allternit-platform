// :feature:automation — Automation Tasks (handoffs/android-native-2026-11-18, wave F2). A port of iOS
// Features/Automation: the hub (goals, routines, loops, agent heartbeats as list, graph and calendar),
// the task detail, the create wizard and scheduled-task sheets, the computer's own routines and loops,
// and the Tasks board. The routine time editor and cron rule come from :feature:bots (wave E).
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.automation"
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
    // ChatBackend.runtimeJson: the relay to the computer's gizzi-code (v1/automations, v1/cron) and Factory board.
    api(project(":feature:chat"))
    // LibraryNet (the /api/v1 routes), the JSON helpers and the library chrome.
    implementation(project(":feature:artifacts"))
    // RoutineRule + RoutineScheduleEditor (the time editor) and BotsApi.listAgents (the bot picker).
    implementation(project(":feature:bots"))
    implementation(project(":core:auth"))
    implementation(libs.activity.compose)
    testImplementation(libs.junit)
}
