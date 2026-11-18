// On-device models engine for the Allternit Android app (plan
// PLAN-on-device-models-2026-11-15, step W9). Shipped in the "play" flavor only
// (app/build.gradle.kts: playImplementation(project(":ondevice"))).
//
//  - llama.cpp (pinned tag, see src/main/cpp/CMakeLists.txt) through JNI, arm64-v8a.
//    Backends load at runtime from the APK's native lib dir: every CPU variant
//    (armv8.0 up to i8mm/sve) and OpenCL for Adreno. If the phone has no usable
//    OpenCL driver the OpenCL backend just fails to load and the CPU one runs.
//  - EngineService: the "running on this phone" notification with Stop (no foreground service since 0.7.2).
//  - ModelDownloads: DownloadManager (system download notification, any network),
//    sha256 checked against the Freeweights manifest, stored in app-private files.
//  - TwaBridge: the TWA postMessage channel to m.allternit.com (spike S1 verdict).
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.allternit.ondevice"
    compileSdk = 36
    // Already on the build Macs (Homebrew android-commandlinetools). Phone Link's
    // mesh AAR uses the same NDK.
    ndkVersion = "27.2.12479018"

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
        ndk {
            // The Play app targets phones. 32-bit ARM and x86 are not built: the
            // engine needs 64-bit for model mmap sizes, and Play serves per-ABI splits.
            abiFilters += listOf("arm64-v8a")
        }
        externalNativeBuild {
            cmake {
                arguments += listOf(
                    "-DCMAKE_BUILD_TYPE=Release",
                    "-DANDROID_STL=c++_shared",
                    // Google Play requires 16 KB page support: link every library (our
                    // wrapper and the fetched llama.cpp/ggml ones) with 16 KB-aligned LOAD
                    // segments. NDK r28 does this by default; r27 needs the switch.
                    "-DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON",
                    // Where FetchContent keeps the pinned sources (gitignored). Shared by
                    // every variant so the 39 MB llama.cpp tarball is fetched once.
                    "-DFETCHCONTENT_BASE_DIR=${project.projectDir}/.deps",
                )
            }
        }
    }

    externalNativeBuild {
        cmake {
            path("src/main/cpp/CMakeLists.txt")
            // CMake from PATH (Homebrew). AGP looks there when the SDK has no
            // matching cmake package.
            version = "3.31.0+"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
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

    packaging {
        jniLibs {
            // Built only to link against (Khronos ICD loader). At run time the
            // phone's own /vendor/lib64/libOpenCL.so is used (declared with
            // <uses-native-library> in the manifest). Packaging ours would hide it.
            excludes += "**/libOpenCL.so"
        }
    }
}

dependencies {
    // The TWA launcher (LauncherActivity, TwaLauncher, DelegationService). Apache-2.0.
    implementation("com.google.androidbrowserhelper:androidbrowserhelper:2.5.0")
    // postMessage with a target origin (requestPostMessageChannel(source, target,
    // extras)) needs androidx.browser 1.6+; androidbrowserhelper 2.5.0 brings 1.4.0.
    implementation("androidx.browser:browser:1.8.0")
    // Ed25519 + BLAKE2b for the Freeweights minisign check (lightweight API only, no provider
    // registration, so it does not clash with Android's built-in "BC"). java.security Ed25519 needs API 33.
    implementation("org.bouncycastle:bcprov-jdk18on:1.78.1")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
