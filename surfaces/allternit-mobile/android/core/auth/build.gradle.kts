// :core:auth — native Android app foundation (handoffs/android-native-2026-11-18, step 4).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.allternit.core.auth"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // TokenProvider is a BearerProvider, so an auth implementation plugs straight
    // into the :core:api client factory.
    api(project(":core:api"))
}

// Kotlin 2.2+ rejects android { kotlinOptions { } } (deprecation raised to error);
// compilerOptions is the replacement and also works on older Kotlin.
kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
