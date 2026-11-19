// :feature:onboarding — native sign-in (handoffs/android-native-2026-11-18, step 7b).
// The twin of iOS LoginGateView (ios/App/AllternitApp.swift). Only :app's nativeapp
// flavor reaches this module (through :feature:shell).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.onboarding"
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
    api(project(":core:auth"))
}
