plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.allternit.app"
    compileSdk = 36

    defaultConfig {
        // Since 0.4.0. Phone Link Kotlin classes keep the com.allternit.phonelink package (src/phonelink).
        applicationId = "com.allternit.app"
        minSdk = 26
        targetSdk = 36
        versionCode = 14
        versionName = "0.5.4"
    }

    // Two builds of the same app ID:
    //  - phonelink: the sideloaded APK Allternit Desktop installs (allternit.com/android).
    //    The Allternit app plus Phone Link (src/phonelink: code, mesh library,
    //    accessibility service, services, receivers, permissions).
    //  - play: the Google Play bundle. Only the Allternit app (src/main). No
    //    Phone Link code, library or permission is packaged.
    flavorDimensions += "dist"
    productFlavors {
        create("phonelink") {
            dimension = "dist"
            isDefault = true
            // The stock androidbrowserhelper launcher (no in-app review here:
            // this build is not installed from Google Play).
            manifestPlaceholders["launcherActivity"] =
                "com.google.androidbrowserhelper.trusted.LauncherActivity"
        }
        create("play") {
            dimension = "dist"
            // Subclass of LauncherActivity that may show the Play in-app review
            // sheet before opening the TWA (src/play/java/.../ReviewLauncherActivity.kt).
            manifestPlaceholders["launcherActivity"] = "com.allternit.app.ReviewLauncherActivity"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
        // SPIKE S1 branch only: debug installs as com.allternit.app.s1 so it sits
        // next to the Play-installed com.allternit.app on the test phone instead
        // of replacing it. Not for main.
        getByName("debug") {
            applicationIdSuffix = ".s1"
        }
    }

    // SPIKE S1 (Android bridge, plan PLAN-on-device-models-2026-11-15): debug
    // builds only. The "debug" source set is merged into phonelinkDebug and
    // playDebug and never into a release build. Everything lives in
    // spike/s1-bridge/ (see its README). Delete this block with the spike.
    sourceSets {
        getByName("debug") {
            manifest.srcFile("../spike/s1-bridge/AndroidManifest.xml")
            java.srcDir("../spike/s1-bridge/src")
            assets.srcDir("../spike/s1-bridge/web")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }

    // The mesh library is ~21 MB per ABI uncompressed. Compressing it in the APK
    // (extracted once at install) keeps the download near 15 MB instead of ~43 MB.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }

    lint {
        // The protected receiver/service use android.permission.DUMP on purpose
        // (only adb shell / system hold it); lint flags system permissions.
        disable += setOf("ProtectedPermissions")
    }
}

dependencies {
    // No runtime dependencies beyond the Kotlin stdlib: the HTTP server, the
    // envelope crypto and X25519 are in-tree (see README "Dependencies").
    // The mesh node is the gomobile-bound tsnet wrapper from
    // infrastructure/mesh/tsnet-ios — build it first:
    //   infrastructure/mesh/tsnet-ios/build-android.sh   (output: Mesh.aar,
    //   copied to app/libs/, gitignored; ARM only (arm64-v8a, armeabi-v7a), stripped, ~15 MB)
    "phonelinkImplementation"(files("libs/Mesh.aar"))
    // The Allternit app as a Trusted Web Activity around m.allternit.com
    // (LauncherActivity, Custom Tab fallback). Apache-2.0, Google.
    implementation("com.google.androidbrowserhelper:androidbrowserhelper:2.5.0")
    // Play In-App Review (the "rate this app" sheet), Google Play build only.
    // The web content cannot call it, so ReviewLauncherActivity asks natively.
    "playImplementation"("com.google.android.play:review:2.0.2")
    // SPIKE S1, debug builds only: the TWA postMessage API with a target origin
    // needs androidx.browser 1.6+; androidbrowserhelper 2.5.0 brings 1.4.0.
    debugImplementation("androidx.browser:browser:1.8.0")
    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM tests (android.jar only ships stubs). Public domain
    // since 20231013.
    testImplementation("org.json:json:20240303")
}
