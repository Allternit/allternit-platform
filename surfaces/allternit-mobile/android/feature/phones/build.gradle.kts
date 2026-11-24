// :feature:phones — Phones and Connect (wave F4a). A port of iOS Features/{Phones,Connect} and Shell/PhonesSheet: the Phones sheet, linked phones, app phones settings, the live phone view (H.264 over the relay), phone tasks, Android screen pairing, the connect wizard and the vendor connect.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.phones"
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
    // ChatBackend (the relay to the user's computer, the runtime socket).
    api(project(":feature:chat"))
    // LibraryNet (the /api/v1 routes), the JSON helpers and the library chrome.
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    implementation(libs.activity.compose)
    // Bot records, channels (BotsApi) for the Phones sheet and the connect flows.
    implementation(project(":feature:bots"))
    // CloudPhone + AciApi (the linked phones list), shared with ACI Computers.
    implementation(project(":feature:computer"))
    testImplementation(libs.junit)
}
