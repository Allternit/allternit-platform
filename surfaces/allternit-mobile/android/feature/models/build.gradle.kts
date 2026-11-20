// :feature:models — Models (wave F4a). A port of iOS Features/Models: the Models tab, Benchmark, Freeweights browse and the Model Lab section.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.models"
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
    // ChatStreamEvent (the benchmark times a streamed reply).
    implementation(project(":core:stream"))
    // Gap run GS-D: the on-phone llama.cpp engine, downloads and storage ("On this phone").
    implementation(project(":ondevice"))
    testImplementation(libs.junit)
}
