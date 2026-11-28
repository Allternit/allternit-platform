// :feature:chat — the native chat screen (handoffs/android-native-2026-11-18, wave A).
// ChatScreen + ChatViewModel, a port of iOS Features/Chat: send and stream a turn, history,
// abort, edit-resend, the message cards, permission asks and the no-computer state.
// :feature:shell depends on it; only :app's nativeapp flavor ships it.
import java.security.MessageDigest

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    // EvalBackend's @Serializable models (eval loop test cases).
    alias(libs.plugins.kotlin.serialization)
}

// sherpa-onnx's Android AAR (version in gradle/libs.versions.toml, repository in settings.gradle.kts).
val sherpaOnnxAar = "com.k2fsa.sherpa.onnx:sherpa-onnx:${libs.versions.sherpaOnnx.get()}@aar"

android {
    namespace = "com.allternit.feature.chat"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
        // sherpa-onnx fills its config data classes over JNI by field name; keep them if the app ever minifies.
        consumerProguardFiles("consumer-rules.pro")
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
    // Theme, FriendlyState, ThinkingOrb, Haptics and the Compose BOM.
    api(project(":core:design"))
    // The generated client (sessions, messages) and ApiCall for the hand-written routes.
    api(project(":core:api"))
    // POST /api/agent-chat frames (AgentChatStream, ChatStreamEvent).
    implementation(project(":core:stream"))
    // AllternitAuth.tokenProvider for the instance and upload calls.
    implementation(project(":core:auth"))
    // rememberLauncherForActivityResult (save a run_code file where the person picks).
    implementation(libs.activity.compose)
    // In-app Custom Tab for URL-mode MCP questions.
    implementation(libs.androidx.browser)
    // Wave H1: Cloud credits and the store sheet (Google Play Billing) for the credits and compute gates.
    implementation(project(":feature:billing"))
    // Gap run GS-D: chat turns on the phone (ondevice/<id>#<quant> brains).
    implementation(project(":ondevice"))
    // Gap run GS-D: the own-key voice engine reads the Keystore-sealed voice keys (SealedSecret).
    implementation(project(":core:files"))
    // On-phone voice (iOS Core/Voice LocalVoiceEngine + VoicePackManager): sherpa-onnx runs Silero VAD, Moonshine and
    // Kokoro; Commons Compress unpacks the .tar.bz2 packs. The packs download at run time, never into the APK.
    implementation(sherpaOnnxAar)
    implementation(libs.commons.compress)
    implementation(libs.kotlinx.serialization.json)
    testImplementation(libs.junit)
}

// The sherpa-onnx AAR comes from a GitHub release (settings.gradle.kts), not a signed Maven repository, so its bytes
// are checked against the sha256 GitHub lists for the v1.13.8 asset before anything compiles against it.
val sherpaOnnxSha256 = "633c24321e06b1fe79feafa03ea16cbc0f8a286641e2da3559bac91bdb13bd96"
val sherpaOnnxFiles: FileCollection = files(configurations.detachedConfiguration(dependencies.create(sherpaOnnxAar)))
val verifySherpaOnnx by tasks.registering {
    inputs.files(sherpaOnnxFiles)
    val expected = sherpaOnnxSha256
    val aar = sherpaOnnxFiles
    doLast {
        val file = aar.singleFile
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buf = ByteArray(1 shl 20)
            while (true) { val n = input.read(buf); if (n < 0) break; digest.update(buf, 0, n) }
        }
        val actual = digest.digest().joinToString("") { b -> "%02x".format(b) }
        check(actual == expected) { "sherpa-onnx AAR ${file.name}: sha256 $actual, expected $expected" }
    }
}
tasks.named("preBuild") { dependsOn(verifySherpaOnnx) }
