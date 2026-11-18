// :core:api — native Android app foundation (handoffs/android-native-2026-11-18, step 4).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "com.allternit.core.api"
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
    // OkHttp (Apache-2.0, Square), the same version :app already ships.
    api(libs.okhttp)
    // kotlinx.serialization (Apache-2.0, JetBrains): the generated models are @Serializable.
    api(libs.kotlinx.serialization.json)
    // kotlinx.coroutines (Apache-2.0, JetBrains): suspend calls with cancellation.
    api(libs.kotlinx.coroutines.core)
    testImplementation(libs.junit)
}
