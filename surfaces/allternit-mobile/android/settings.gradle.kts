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
