pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // sherpa-onnx's Android AAR (Apache-2.0) is published only as a k2-fsa GitHub release asset:
        // v<version>/sherpa-onnx-<version>.aar. Only that one module resolves here; :feature:chat verifies its sha256.
        ivy {
            name = "sherpaOnnxReleases"
            url = uri("https://github.com/k2-fsa/sherpa-onnx/releases/download")
            patternLayout { artifact("v[revision]/[artifact]-[revision].[ext]") }
            metadataSources { artifact() }
            content { includeModule("com.k2fsa.sherpa.onnx", "sherpa-onnx") }
        }
    }
}
rootProject.name = "allternit-phone-link"
include(":app")
// On-device models engine (llama.cpp via JNI + the TWA bridge). Shipped in the
// "play" flavor only: app/build.gradle.kts adds it as playImplementation.
include(":ondevice")
// Native app foundation (handoffs/android-native-2026-11-18, step 4). Library modules
// only: :app does not depend on them yet, so the TWA builds are unchanged.
include(":core:design")
include(":core:api")
include(":core:auth")
include(":core:stream")
include(":core:data")
// Gap run GS-B: the shared Files key crypto, Keystore boxes and the voice hand-off crypto. :app depends on it in every
// flavor (it has no Compose or Clerk); :feature:settings, :feature:computer and :feature:push use it too.
include(":core:files")
// Native app screens (step 7b). Only the "nativeapp" flavor of :app depends on them, so the
// play and phonelink (TWA) builds do not compile or ship any of this.
include(":feature:onboarding")
include(":feature:shell")
// Wave A: the chat screen. :feature:shell depends on it (nativeapp flavor only, like the shell).
include(":feature:chat")
// Wave D: projects and artifacts. :feature:shell depends on both (nativeapp flavor only); :feature:projects
// uses :feature:artifacts for the library chrome and to open a project's artifacts.
include(":feature:artifacts")
include(":feature:projects")
// Wave E: Bots mode. :feature:shell depends on it (nativeapp flavor only); it reuses :feature:chat and
// :feature:artifacts (LibraryNet, library chrome).
include(":feature:bots")
// Wave F1: Code mode. :feature:shell depends on it (nativeapp flavor only); it reuses :feature:chat
// (the relay: runtime calls, the pty socket) and :feature:artifacts (LibraryNet, library chrome).
include(":feature:code")
// Wave F2: Automation Tasks. :feature:shell depends on it (nativeapp flavor only); it reuses :feature:bots
// (the routine time editor and rule), :feature:chat (the relay) and :feature:artifacts (LibraryNet, chrome).
include(":feature:automation")
// Wave F3: ACI mode and the native RFB viewer. :core:rfb (the VNC client, the screen canvas, the computer
// drive screen) is shared by :feature:computer (ACI Computers) and :feature:bots (a bot's desktop).
include(":core:rfb")
include(":feature:computer")
// Wave F4a: Cowork, Models, and Phones + Connect. :feature:shell depends on all three (nativeapp flavor only).
include(":feature:cowork")
include(":feature:models")
include(":feature:phones")
// Wave F4b: Allternit Office (the floor, documents, the editor web view, PDF signing, local documents). :feature:shell
// depends on it (nativeapp flavor only); it reuses :feature:bots (the roster), :feature:chat and :feature:artifacts.
include(":feature:office")
// Wave G1: Settings part 1 (the root, Advanced, Account, Privacy, Notifications, Memory, Devices, Models, Subagents,
// MCP, Webhooks). :feature:shell depends on it (nativeapp flavor only).
include(":feature:settings")
// Wave H1: purchases and billing (Google Play Billing, the store sheet, Cloud credits). :feature:settings, :feature:chat
// and :feature:shell depend on it (nativeapp flavor only), so Play Billing ships in the native app and nowhere else.
include(":feature:billing")
// Wave H2: Meetings (the agenda, live meeting, recap). :feature:shell depends on it (nativeapp flavor only).
include(":feature:meetings")
// Wave I1: push notifications (FCM), channels and notification buttons.
include(":feature:push")
// Wave I2: phone system surfaces (Glance widget, Quick Settings tiles, status sync, share target). :feature:push, :feature:shell
// depend on it (nativeapp flavor only).
include(":feature:system")
