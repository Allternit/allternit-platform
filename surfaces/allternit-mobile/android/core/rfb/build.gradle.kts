// :core:rfb — the native RFB (VNC) client (handoffs/android-native-2026-11-18, wave F3). A hand port of
// iOS Features/Agents/Desktop/RFB{Protocol,Auth,Decoder,Client,CredentialsForm}.swift with platform APIs
// only: OkHttp's WebSocket (the app's existing transport), java.util.zip.Inflater for ZRLE, javax.crypto
// for the VNC / Apple sign-ins, Bitmap + Compose Canvas for the framebuffer. It also holds the shared
// screen canvas (pinch-zoom, pan, taps, keyboard) and the computer drive screen, so ACI Computers
// (:feature:computer) and a bot's desktop (:feature:bots) use one viewer, as on iOS.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.core.rfb"
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
    api(project(":core:api"))
    implementation(project(":core:auth"))
    implementation(libs.okhttp)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.serialization.json)
    testImplementation(libs.junit)
}
