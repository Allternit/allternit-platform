// :feature:shell — the native MainActivity (handoffs/android-native-2026-11-18, step 7b).
// Sign-in gate, then a placeholder home (drawer and mode dial: step 7c). Only :app's nativeapp
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
    // setContent { } for MainActivity.
    implementation(libs.activity.compose)
}
