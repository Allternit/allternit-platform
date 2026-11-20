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

// Wave I1 (push): the Firebase client options as the string resources FirebaseInitProvider reads (google_app_id,
// google_api_key, gcm_defaultSenderId, project_id), for the nativeapp flavor only. No google-services plugin and
// nothing Firebase-specific in git. Read from the ALLTERNIT_FIREBASE_APP_ID / _API_KEY / _SENDER_ID / _PROJECT_ID
// gradle properties or env vars, else from the local client config file (ALLTERNIT_FIREBASE_CONFIG, default
// ~/.config/allternit/firebase/google-services.json, client com.allternit.app). Without either, the app builds and
// runs with push off (FirebaseInitProvider logs that it found no options; AllternitPush.isConfigured is false).
val firebaseResValues: Map<String, String> = run {
    fun setting(name: String): String? =
        (providers.gradleProperty(name).orNull ?: providers.environmentVariable(name).orNull)?.takeIf { it.isNotBlank() }
    val explicit = mapOf(
        "google_app_id" to setting("ALLTERNIT_FIREBASE_APP_ID"),
        "google_api_key" to setting("ALLTERNIT_FIREBASE_API_KEY"),
        "gcm_defaultSenderId" to setting("ALLTERNIT_FIREBASE_SENDER_ID"),
        "project_id" to setting("ALLTERNIT_FIREBASE_PROJECT_ID"),
    )
    if (explicit.values.all { it != null }) return@run explicit.mapValues { it.value.orEmpty() }
    val configFile = project.file(setting("ALLTERNIT_FIREBASE_CONFIG") ?: "${System.getProperty("user.home")}/.config/allternit/firebase/google-services.json")
    if (!configFile.isFile) return@run emptyMap()
    val root = groovy.json.JsonSlurper().parse(configFile) as? Map<*, *> ?: return@run emptyMap()
    val project = root["project_info"] as? Map<*, *> ?: return@run emptyMap()
    val client = (root["client"] as? List<*>).orEmpty().filterIsInstance<Map<*, *>>().firstOrNull { c ->
        ((c["client_info"] as? Map<*, *>)?.get("android_client_info") as? Map<*, *>)?.get("package_name") == "com.allternit.app"
    } ?: return@run emptyMap()
    val values = mapOf(
        "google_app_id" to ((client["client_info"] as? Map<*, *>)?.get("mobilesdk_app_id") as? String),
        "google_api_key" to (((client["api_key"] as? List<*>)?.firstOrNull() as? Map<*, *>)?.get("current_key") as? String),
        "gcm_defaultSenderId" to project["project_number"]?.toString(),
        "project_id" to (project["project_id"] as? String),
    )
    if (values.values.any { it.isNullOrBlank() }) emptyMap() else values.mapValues { it.value.orEmpty() }
}

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
        versionCode = 19
        versionName = "0.7.3"
    }

    // Two builds of the same app ID:
    //  - phonelink: the sideloaded APK Allternit Desktop installs (allternit.com/android).
    //    The Allternit app plus Phone Link (src/phonelink: local server, mesh
    //    library, adb shell server for scrcpy, services, receivers, permissions)
    //    plus the screen-share lane (src/phoneshare) as the fallback when the
    //    scrcpy shell server isn't running.
    //  - play: the Google Play bundle. The Allternit app (src/main), Play Billing and the
    //    on-device engine. No src/phoneshare (no screen share, no Bluetooth control, no
    //    Accessibility service, no foreground services: 0.7.2, 2026-11-18). None of the adb
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
            manifestPlaceholders["twaLauncherCategory"] = "android.intent.category.LAUNCHER"
        }
        create("play") {
            dimension = "dist"
            // Subclass of LauncherActivity that may show the Play in-app review
            // sheet before opening the TWA (src/play/java/.../ReviewLauncherActivity.kt).
            manifestPlaceholders["launcherActivity"] = "com.allternit.app.ReviewLauncherActivity"
            manifestPlaceholders["twaLauncherCategory"] = "android.intent.category.LAUNCHER"
        }
        // The native Kotlin + Compose app (handoffs/android-native-2026-11-18, step 7b).
        // Same app ID and signing key. NOT uploaded to Play before the cutover: it is tested by
        // sideload (adb install) on a phone/emulator, then replaces the TWA on Play production
        // with a staged rollout when it reaches parity (PLAN.md section 3): versionCode 1000+ so it always sorts above the TWA builds,
        // which keep 17-999 for hotfixes. Launcher = com.allternit.feature.shell.MainActivity
        // (src/nativeapp/AndroidManifest.xml). The stock TWA LauncherActivity stays in the
        // build without the launcher category, as the "Use web version" target. No Play
        // Billing, no :ondevice, no Phone Link yet (steps 16 and 18).
        create("nativeapp") {
            dimension = "dist"
            versionCode = 1000
            versionNameSuffix = "-native"
            // Wave I1: Firebase options for push (see firebaseResValues above); none → push off.
            firebaseResValues.forEach { (name, value) -> resValue("string", name, value) }
            manifestPlaceholders["launcherActivity"] =
                "com.google.androidbrowserhelper.trusted.LauncherActivity"
            manifestPlaceholders["twaLauncherCategory"] = "android.intent.category.DEFAULT"
        }
    }

    // src/phoneshare is compiled into the phonelink flavor only (no third flavor, no library
    // module: its classes keep the com.allternit.phonelink package and app R).
    // Each flavor's manifest declares the components (an Android source set has
    // one manifest).
    sourceSets {
        getByName("phonelink") {
            java.srcDir("src/phoneshare/java")
            res.srcDir("src/phoneshare/res")
        }
        getByName("testPhonelink") { java.srcDir("src/testPhoneshare/java") }
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
    // with OkHttp (Apache-2.0, Square), both builds. OkHttp 5 from the version catalog
    // (step 7b): the same version :core:api and Clerk Android resolve.
    implementation(libs.okhttp)
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
    // The native app (step 7b): MainActivity, sign-in (Clerk) and the screens behind them.
    // nativeapp flavor only, so play and phonelink compile none of it.
    "nativeappImplementation"(project(":feature:shell"))
    // Wave I2: the widget, tiles and share target classes the nativeapp manifest declares (also reached through :feature:shell).
    "nativeappImplementation"(project(":feature:system"))
    // Files key crypto (files/FilesKeyCrypto.kt): X25519, HKDF-SHA256 and
    // ChaCha20-Poly1305 from BouncyCastle (MIT), the same version :ondevice uses.
    implementation("org.bouncycastle:bcprov-jdk18on:1.78.1")
    // Gap run GS-B: the Files key crypto, key ring and session hook live in :core:files (every flavor), shared with
    // the native Settings and computer screens. No Compose or Clerk inside, so play and phonelink stay as they were.
    implementation(project(":core:files"))
    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM tests (android.jar only ships stubs). Public domain
    // since 20231013.
    testImplementation("org.json:json:20240303")
}

// Kotlin 2.2+ rejects android { kotlinOptions { } } (deprecation raised to error);
// compilerOptions is the replacement and also works on older Kotlin.
kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
