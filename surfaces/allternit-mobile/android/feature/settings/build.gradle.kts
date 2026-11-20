// :feature:settings: Settings part 1 (wave G1). A port of iOS Features/Settings: the Settings root, Advanced, and the
// Account, Privacy, Notifications, Memory, Device, Models, Subagents, MCP and Webhooks pages.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.settings"
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
    // ChatBackend (the relay to the user's computer: MCP servers).
    api(project(":feature:chat"))
    // LibraryNet (the /api/v1 routes) and the JSON helpers.
    implementation(project(":feature:artifacts"))
    implementation(project(":core:auth"))
    // In-app Custom Tab for links (iOS SafariView).
    implementation(libs.androidx.browser)
    testImplementation(libs.junit)
}
