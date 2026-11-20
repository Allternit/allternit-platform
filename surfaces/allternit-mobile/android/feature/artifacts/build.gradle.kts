// :feature:artifacts — the native artifacts screens (handoffs/android-native-2026-11-18, wave D).
// A port of iOS Features/Artifacts: the library, details, the sandboxed / live / page web views,
// the studio, the V2 artifact view and editor, and the PDF template sheet.
// :feature:shell and :feature:projects depend on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.artifacts"
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
    // Theme, FriendlyState, Haptics, the deck_* kind glyphs and the Compose BOM.
    api(project(":core:design"))
    // ApiCall / ApiResult / ApiJson for the hand-written /api/v1/artifacts routes.
    api(project(":core:api"))
    // AllternitAuth.tokenProvider: the bearer for the artifact calls.
    implementation(project(":core:auth"))
    // BackHandler on the full-screen viewers.
    implementation(libs.activity.compose)
    // WebViewCompat.addWebMessageListener / addDocumentStartJavaScript, scoped to the web app origin.
    implementation(libs.androidx.webkit)
    testImplementation(libs.junit)
}
