plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.allternit.phonelink"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.allternit.phonelink"
        minSdk = 26
        targetSdk = 35
        versionCode = 6
        versionName = "0.3.2"
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
    implementation(files("libs/Mesh.aar"))
    testImplementation("junit:junit:4.13.2")
    // Real org.json for JVM tests (android.jar only ships stubs). Public domain
    // since 20231013.
    testImplementation("org.json:json:20240303")
}
