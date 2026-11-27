// :feature:office — Allternit Office (wave F4b). A port of iOS Features/Office: the Office floor (the 3D facility fed with the
// person's bots), the Office documents list, the sectioned document viewer, the interactive editor web view, PDF signing and the
// local document viewer. :feature:shell depends on it; only :app's nativeapp flavor ships it.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.allternit.feature.office"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
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
    api(project(":core:design"))
    api(project(":core:api"))
    implementation(project(":core:auth"))
    // ChatViewModel (the floor panel's chat), ChatBackend.
    implementation(project(":feature:chat"))
    // LibraryNet, library chrome, ArtifactsV2Client, the artifact hooks.
    implementation(project(":feature:artifacts"))
    // The roster and threads behind the floor.
    implementation(project(":feature:bots"))
    implementation(libs.activity.compose)
    // WebViewCompat.addWebMessageListener / addDocumentStartJavaScript.
    implementation(libs.androidx.webkit)
    // Real-PDF signing (PdfSign.kt): keeps the original pages and their text.
    // PdfBox pulls BouncyCastle *-jdk15to18, which ships the same org.bouncycastle classes as the
    // bcprov-jdk18on that :core:files, :app and :ondevice use (duplicate-class failure in :app).
    // Drop its copy and give it the jdk18on build of the same artifacts instead.
    implementation(libs.pdfbox.android) {
        exclude(group = "org.bouncycastle")
    }
    implementation("org.bouncycastle:bcprov-jdk18on:1.78.1")
    implementation("org.bouncycastle:bcpkix-jdk18on:1.78.1")
    testImplementation(libs.junit)
}
