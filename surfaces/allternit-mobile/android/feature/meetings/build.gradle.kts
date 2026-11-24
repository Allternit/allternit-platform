// :feature:meetings: Meetings (wave H2). A port of iOS Features/Calls/Meetings: the agenda, the live meeting
// and the recap. Gizzi joins calls from the person's own computer (or their cloud computer), never from
// Allternit's servers. :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.meetings"
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
    // The voice speaker ring (VoiceSpeakerRing) shared with voice mode.
    implementation(project(":feature:chat"))
    // LibraryNet (the account token client and the cloud-api base URL).
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    // The store sheet for the paid-plan card (Google Play Billing).
    implementation(project(":feature:billing"))
    implementation(project(":feature:push"))
    // BackHandler.
    implementation(libs.activity.compose)
    // In-app Custom Tab for call links and meeting artifacts (iOS openURL).
    implementation(libs.androidx.browser)
}
