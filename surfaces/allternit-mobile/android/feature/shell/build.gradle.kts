// :feature:shell — the native MainActivity (handoffs/android-native-2026-11-18, step 7b).
// Sign-in gate, then the home shell (drawer, mode fan: step 7c-1). Only :app's nativeapp
// flavor depends on it; the play and phonelink TWA builds never see it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.shell"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    buildFeatures {
        compose = true
        // BuildConfig.DEBUG gates the Maestro ticket sign-in (never in release).
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
    implementation(project(":feature:onboarding"))
    // Wave A: the chat screen that replaces the shell's chat placeholder.
    implementation(project(":feature:chat"))
    // Wave D: Projects and Artifacts replace the drawer rows' placeholders.
    implementation(project(":feature:projects"))
    implementation(project(":feature:artifacts"))
    // Wave E: Bots mode replaces the Bots placeholder.
    implementation(project(":feature:bots"))
    // Wave F1: Code mode replaces the Code placeholder.
    implementation(project(":feature:code"))
    implementation(project(":feature:automation"))
    // Wave F3: ACI mode (the ACI tab, browser, agent run, Computers).
    implementation(project(":feature:computer"))
    // Wave F4a: Cowork, Models, Phones + Connect.
    implementation(project(":feature:cowork"))
    implementation(project(":feature:models"))
    implementation(project(":feature:phones"))
    implementation(project(":feature:office"))
    // Wave G1: Settings part 1 replaces the Settings stub.
    implementation(project(":feature:settings"))
    // Wave H1: Google Play Billing starts at launch and posts open purchases (PlayPurchases.start).
    implementation(project(":feature:billing"))
    // Wave H2: Meetings (the drawer entry and the Meetings screens).
    implementation(project(":feature:meetings"))
    // Wave I1: push notifications (FCM token registration, channels, taps).
    implementation(project(":feature:push"))
    // Wave I2: status snapshot refresh on foreground, the system links (widget, tiles, shortcuts).
    implementation(project(":feature:system"))
    // The drawer and home list GET /agent-sessions (also reaches us through :core:auth's api()).
    implementation(project(":core:api"))
    // Gap run GS-B: FilesSession (the Files drive signs in with the native session).
    implementation(project(":core:files"))
    // Gap run GS-D: the brain picker lists the models on this phone (:ondevice PhoneModels).
    implementation(project(":ondevice"))
    // Wave B: DataStore for the first-run "setup done" flag (iOS OnboardingStore, UserDefaults).
    implementation(project(":core:data"))
    // setContent { } for MainActivity.
    implementation(libs.activity.compose)
}
