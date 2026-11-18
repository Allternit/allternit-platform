plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Release signing from the environment (native app step 4). The build train (or a
// release Mac) sets all four; when any is missing the release build stays unsigned,
// exactly as before, and is signed afterwards. No path or password lives in the repo.
val releaseSigningEnv: Map<String, String>? = listOf(
    "ALLTERNIT_ANDROID_KEYSTORE",
    "ALLTERNIT_ANDROID_KEYSTORE_PASSWORD",
    "ALLTERNIT_ANDROID_KEY_ALIAS",
    "ALLTERNIT_ANDROID_KEY_PASSWORD",
).associateWith { providers.environmentVariable(it).orNull.orEmpty() }
    .takeIf { env -> env.values.all { it.isNotBlank() } }

android {
    namespace = "com.allternit.app"
    compileSdk = 36

    defaultConfig {
        // Since 0.4.0. Phone Link Kotlin classes keep the com.allternit.phonelink package (src/phonelink).
        applicationId = "com.allternit.app"
        minSdk = 26
        targetSdk = 36
        // One versionCode for both builds (same app ID): the Play bundle and the
        // sideloaded APK move together.
        versionCode = 16
        versionName = "0.7.0"
    }

    // Two builds of the same app ID:
    //  - phonelink: the sideloaded APK Allternit Desktop installs (allternit.com/android).
    //    The Allternit app plus Phone Link (src/phonelink: local server, mesh
    //    library, adb shell server for scrcpy, services, receivers, permissions)
    //    plus the screen-share lane (src/phoneshare) as the fallback when the
    //    scrcpy shell server isn't running.
    //  - play: the Google Play bundle. The Allternit app (src/main) plus the
    //    screen-share lane (src/phoneshare: claim code + owner pairing, MediaProjection
    //    capture, the Accessibility service for a viewer's taps). None of the adb
    //    pieces (ShellServerLauncher, ShellServerMain, AdbClient/AdbCert/AdbKeyBox,
    //    WRITE_SECURE_SETTINGS, KILL_BACKGROUND_PROCESSES), no local server, no mesh.
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

    // src/phoneshare is compiled into both flavors (no third flavor, no library
    // module: its classes keep the com.allternit.phonelink package and app R).
    // Each flavor's manifest declares the components (an Android source set has
    // one manifest); FlavorAccessibility is defined once per flavor.
    sourceSets {
        getByName("phonelink") {
            java.srcDir("src/phoneshare/java")
            res.srcDir("src/phoneshare/res")
        }
        getByName("play") {
            java.srcDir("src/phoneshare/java")
            res.srcDir("src/phoneshare/res")
        }
        getByName("testPhonelink") { java.srcDir("src/testPhoneshare/java") }
        getByName("testPlay") { java.srcDir("src/testPhoneshare/java") }
    }

    signingConfigs {
        releaseSigningEnv?.let { env ->
            create("release") {
                storeFile = file(env.getValue("ALLTERNIT_ANDROID_KEYSTORE"))
                storePassword = env.getValue("ALLTERNIT_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = env.getValue("ALLTERNIT_ANDROID_KEY_ALIAS")
                keyPassword = env.getValue("ALLTERNIT_ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (releaseSigningEnv != null) {
                signingConfig = signingConfigs.getByName("release")
            }
            // The static "Inbox tasks" shortcut targets this app's own package.
            resValue("string", "files_app_id", "com.allternit.app")
        }
        // Debug installs as com.allternit.app.ondevice, next to the Play-installed
        // com.allternit.app on a test phone instead of replacing it. Release keeps the real ID.
        getByName("debug") {
            applicationIdSuffix = ".ondevice"
            resValue("string", "files_app_id", "com.allternit.app.ondevice")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // Files drive (both flavors): the contract types are generated into
    // contracts/files-drive/gen by `node contracts/files-drive/generate.mjs` and compiled
    // straight from there, so the app can never drift from the schema. Only the .kt file
    // in that folder is Kotlin; the .ts and .swift outputs beside it are ignored here.
    sourceSets.getByName("main").kotlin.srcDir("../../../../contracts/files-drive/gen")

    testOptions {
        unitTests.isReturnDefaultValues = true
    }

    // The mesh library is ~21 MB per ABI uncompressed. Compressing it in the APK
    // (extracted once at install) keeps the download near 15 MB instead of ~43 MB.
    // The play build's on-device engine needs the extraction too: llama.cpp loads its
    // backends (libggml-cpu-*.so, libggml-opencl.so) by path from nativeLibraryDir.
    packaging {
        jniLibs {
            useLegacyPackaging = true
            // Link-time stub from :ondevice; phones use their own /vendor libOpenCL.so.
            excludes += "**/libOpenCL.so"
        }
        resources {
            // Two dependencies (Play Billing's tree) ship the same OSGi manifest;
            // Android never reads it, and AGP 8.9 refuses the duplicate.
            excludes += "META-INF/versions/9/OSGI-INF/MANIFEST.MF"
        }
    }

    lint {
        // The protected receiver/service use android.permission.DUMP on purpose
        // (only adb shell / system hold it); lint flags system permissions.
        disable += setOf("ProtectedPermissions")
    }
}

dependencies {
    // The HTTP server, the envelope crypto and X25519 are in-tree (see README
    // "Dependencies"). The screen-share lane dials out over HTTPS/WebSockets
    // with OkHttp (Apache-2.0, Square), both builds.
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    // The mesh node is the gomobile-bound tsnet wrapper from
    // infrastructure/mesh/tsnet-ios — build it first:
    //   infrastructure/mesh/tsnet-ios/build-android.sh   (output: Mesh.aar,
    //   copied to app/libs/, gitignored; ARM only (arm64-v8a, armeabi-v7a), stripped, ~15 MB)
    "phonelinkImplementation"(files("libs/Mesh.aar"))
    // The Allternit app as a Trusted Web Activity around m.allternit.com
    // (LauncherActivity, Custom Tab fallback). Apache-2.0, Google. Its androidx.browser
    // (compile scope) also gives the Files sign-in its Custom Tab (files/FilesSignInActivity).
    implementation("com.google.androidbrowserhelper:androidbrowserhelper:2.5.0")
    // Play In-App Review (the "rate this app" sheet), Google Play build only.
    // The web content cannot call it, so ReviewLauncherActivity asks natively.
    "playImplementation"("com.google.android.play:review:2.0.2")
    // Google Play Billing for the TWA (store purchases D15), Google Play build only:
    // the Digital Goods API handler (PlayDelegationService) and the PaymentRequest
    // provider (PaymentActivity / PaymentService). Apache-2.0, Google. 1.2.0 is the
    // newest release (dl.google.com maven-metadata, 2026-11-17); its POM brings Play
    // Billing Library 8.3.0 (new apps and updates need PBL 8+: the PBL 7 deadline was
    // 2026-08-31, its extension 2026-11-01; PBL 8 is accepted until 2027-08-31,
    // developer.android.com/google/play/billing/deprecation-faq) and androidx.browser
    // 1.10.0, which Gradle resolves over :ondevice's 1.8.0 in this build. Its
    // androidbrowserhelper 2.2.2 resolves to our 2.5.0. Never add it to phonelink:
    // that build must not carry com.android.vending.BILLING.
    "playImplementation"("com.google.androidbrowserhelper:billing:1.2.0")
    // On-device models (llama.cpp engine, downloads, TWA postMessage bridge), Google Play
    // build only. See ondevice/README.md. ReviewLauncherActivity extends its
    // BridgedLauncherActivity.
    "playImplementation"(project(":ondevice"))
    // Files key crypto (files/FilesKeyCrypto.kt): X25519, HKDF-SHA256 and
    // ChaCha20-Poly1305 from BouncyCastle (MIT), the same version :ondevice uses.
    implementation("org.bouncycastle:bcprov-jdk18on:1.78.1")
    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM tests (android.jar only ships stubs). Public domain
    // since 20231013.
    testImplementation("org.json:json:20240303")
}
