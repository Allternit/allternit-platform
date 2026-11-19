// :feature:projects — the native projects screens (handoffs/android-native-2026-11-18, wave D).
// A port of iOS Features/Projects: the list (List, Grid, Deck), folder covers and the folder look
// sheet, new project, project detail, the project pad, code project detail and the design canvas.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.projects"
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
    // The library chrome (header, sheets) and the artifact screens a project opens.
    api(project(":feature:artifacts"))
    // Theme, FriendlyState, Haptics, the deck_* kind glyphs and the Compose BOM.
    api(project(":core:design"))
    // ApiCall / ApiResult / ApiJson for the hand-written project routes.
    api(project(":core:api"))
    // AllternitAuth.tokenProvider: the bearer for the artifact calls.
    implementation(project(":core:auth"))
    // BackHandler on the full-screen viewers.
    implementation(libs.activity.compose)
    testImplementation(libs.junit)
}
