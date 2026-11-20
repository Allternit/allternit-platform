// :feature:billing (wave H1): plans and credit packs bought through Google Play Billing, the port of iOS
// Core/Store (StoreBillingClient, StorePurchaseManager), Features/Settings/StorePurchaseViews and
// Features/Shared/CloudCreditsSheet, plus the account plan state (iOS AccountPlanStore, UsageStore).
// Only the nativeapp flavor of :app reaches it (through :feature:settings, :feature:chat and :feature:shell), so
// the Play Billing Library and its com.android.vending.BILLING permission ship in the native app only. The play
// (TWA) flavor keeps androidbrowserhelper billing:1.2.0 for the web's Digital Goods API.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.billing"
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
    // ApiConfig / ApiClients (the /api/v1 host and the bearer interceptor).
    implementation(project(":core:api"))
    // AllternitAuth: the token and the signed-in state.
    implementation(project(":core:auth"))
    implementation(libs.play.billing.ktx)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    testImplementation(libs.junit)
}
