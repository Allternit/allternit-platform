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
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    // TokenProvider is a BearerProvider, so an auth implementation plugs straight
    // into the :core:api client factory.
    api(project(":core:api"))
}
