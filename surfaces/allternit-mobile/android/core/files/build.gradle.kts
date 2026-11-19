// :core:files (gap run GS-B): the Files drive crypto every surface of the Android app shares, the port of iOS
// AllternitFilesKit `Keys/` (FilesKeyCrypto, FilesKeyService) and the voice key hand-off crypto.
// - FilesKeyCrypto + XChaCha20Poly1305 + Blake3: moved here from :app (files/), unchanged, so the DocumentsProvider
//   (:app, every flavor), :feature:settings and :feature:computer run one copy.
// - FilesKeyRing: this phone's X25519 Files key ring and the cloud-api `files-keys` calls (approve, decline, rotate).
// - KeystoreBox: AES-256-GCM under a non-exportable Android Keystore key, StrongBox when the phone has one.
// - FilesSession: the native app's Clerk session for the Files client (no Custom Tab sign-in in nativeapp).
// - VoiceHandoffCrypto: P-256 ECDH + HKDF-SHA256 + AES-256-GCM, iOS Core/Voice/VoiceKeyHandoff.swift.
// No Compose, no Clerk, no network library: :app depends on it in every flavor (play and phonelink included).
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.allternit.core.files"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
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
    // X25519, HKDF-SHA256 and ChaCha20-Poly1305 (BouncyCastle, MIT), the version :app and :ondevice use. `api` so
    // :app's own BouncyCastle imports keep resolving to the same jar.
    api("org.bouncycastle:bcprov-jdk18on:1.78.1")
    testImplementation(libs.junit)
}
