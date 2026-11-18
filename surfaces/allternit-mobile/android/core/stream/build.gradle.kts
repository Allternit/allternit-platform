// :core:stream — native Android app foundation (handoffs/android-native-2026-11-18, step 4).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.allternit.core.stream"
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
    // AgentChatRequest + the request builder come from the generated client.
    api(project(":core:api"))
    // okhttp-sse (Apache-2.0, Square), same version as okhttp.
    implementation(libs.okhttp.sse)
    testImplementation(libs.junit)
}
