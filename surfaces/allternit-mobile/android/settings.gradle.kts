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
