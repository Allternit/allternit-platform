// :feature:system (wave I2): the phone system surfaces, ported from iOS AllternitWidgets, SystemSurfaces/Intents and
// AllternitShare. The status snapshot (GET /api/v1/status/summary, iOS StatusSnapshot + StatusSummaryClient), the Glance
// home-screen widget, the Quick Settings tiles (iOS controls), dynamic shortcuts (iOS App Intents) and the share target.
// Components are declared in :app's nativeapp manifest, so only that flavor ships any of it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.system"
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
    // ApiClients, ApiConfig, ApiResult; AllternitTheme and the compose libraries come through :feature:chat's api().
    implementation(project(":core:api"))
    implementation(project(":core:auth"))
    // ChatBackends (uploads, the runtime relay, createSession): the share target sends through them.
    implementation(project(":feature:chat"))
    // Island bar widget: BarDeepLink, BarJobs, BarFlag (lane 37).
    implementation(project(":feature:bar"))
    implementation(project(":core:design"))
    implementation(libs.activity.compose)
    implementation(libs.glance.appwidget)
    implementation(libs.kotlinx.coroutines.android)
}
