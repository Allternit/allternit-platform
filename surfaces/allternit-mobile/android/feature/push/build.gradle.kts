// :feature:push (wave I1): native push notifications through Firebase Cloud Messaging, the port of iOS
// Core/PushNotificationManager, Core/API/PushTokenClient and Core/NotificationActions. Registers the FCM token with
// cloud-api (POST /api/v1/device-tokens, platform "android"), builds each notification from the data message
// (channel per iOS category, the iOS buttons), runs the buttons in the background and opens the tap in MainActivity.
// Only the nativeapp flavor of :app reaches it (through :feature:shell and :feature:settings).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.allternit.feature.push"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    buildFeatures {
        // BuildConfig.DEBUG for the registration log line.
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
    // ApiConfig, ApiClients, AllternitApi.registerDeviceToken / unregisterDeviceToken.
    implementation(project(":core:api"))
    // AllternitAuth: the token, the signed-in state and the sign-out hook.
    implementation(project(":core:auth"))
    // ChatBackends.runtimeJson: the notification buttons call the inbox and session routes through the computer.
    implementation(project(":feature:chat"))
    implementation(libs.firebase.messaging)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
}
