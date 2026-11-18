// :core:data — native Android app foundation (handoffs/android-native-2026-11-18, step 4).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.allternit.core.data"
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
    // Jetpack DataStore (Apache-2.0): the Android stand-in for iOS UserDefaults.
    api(libs.datastore.preferences)
}
