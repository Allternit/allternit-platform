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
