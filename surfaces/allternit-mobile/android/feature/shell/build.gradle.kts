// :feature:shell — the native MainActivity (handoffs/android-native-2026-11-18, step 7b).
// Sign-in gate, then the home shell (drawer, mode fan: step 7c-1). Only :app's nativeapp
// flavor depends on it; the play and phonelink TWA builds never see it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.shell"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    buildFeatures {
        compose = true
        // BuildConfig.DEBUG gates the Maestro ticket sign-in (never in release).
        buildConfig = true
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
    implementation(project(":feature:onboarding"))
    // Wave A: the chat screen that replaces the shell's chat placeholder.
    implementation(project(":feature:chat"))
    // Wave D: Projects and Artifacts replace the drawer rows' placeholders.
    implementation(project(":feature:projects"))
    implementation(project(":feature:artifacts"))
    // Wave E: Bots mode replaces the Bots placeholder.
    implementation(project(":feature:bots"))
    // The drawer and home list GET /agent-sessions (also reaches us through :core:auth's api()).
    implementation(project(":core:api"))
    // Wave B: DataStore for the first-run "setup done" flag (iOS OnboardingStore, UserDefaults).
    implementation(project(":core:data"))
    // setContent { } for MainActivity.
    implementation(libs.activity.compose)
}
