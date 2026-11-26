// :feature:bar (Island bar, lanes 36/37): the Android shape of the iOS island bar. A Glance widget cannot take typing
// (plan D16), so BarWidget (:feature:system) opens BarActivity, a translucent Compose Activity over the home screen:
// app chip, the tool's sentence with blanks (bottom-sheet picker with recents), the review card with the rule, run.
// The job follows the bar SSE stream (wire form {type, session_id, part}) with polling as the fallback, and is published
// as BarJobs.current (BarJobState) for lane 37's ongoing notification and widget. Calls allternit-api /api/v1/bar/*
// with the ApiClients bearer. BarActivity is declared in this module's manifest (merged where :feature:shell pulls it
// in, nativeapp only); hidden behind the `bar.island` flag (off by default).
// No dependency on :feature:push or :feature:system: lane 37 makes those depend on this module (BarJobs, BarDeepLink).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.bar"
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
    // ApiConfig, ApiClients (bearer), OkHttp and kotlinx-serialization-json come through :core:api's api().
    implementation(project(":core:api"))
    implementation(project(":core:auth"))
    // AllternitTheme; Compose runtime, ui, foundation and material3 through its api().
    implementation(project(":core:design"))
    implementation(libs.activity.compose)
    implementation(libs.kotlinx.coroutines.android)
}
