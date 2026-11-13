import java.util.Base64

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

/** Release inputs come from the environment (CI) or ~/.gradle/gradle.properties (local), never the repository. */
fun setting(name: String): String? =
    (providers.environmentVariable(name).orNull ?: providers.gradleProperty(name).orNull)?.takeIf { it.isNotBlank() }

/** docs/release.md: major×1 000 000 + minor×10 000 + patch×100 + n, where n is the pre-release number or 99. */
fun versionCodeOf(version: String): Int {
    val match = Regex("""(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z]+\.(\d+))?""").matchEntire(version)
        ?: error("LAGOM_VERSION '$version' must look like 1.2.3 or 1.2.3-beta.4")
    val (major, minor, patch, pre) = match.destructured
    val n = pre.ifEmpty { "99" }.toInt()
    check(minor.toInt() <= 99 && patch.toInt() <= 99 && n <= 99 && (pre.isEmpty() || n < 99)) {
        "LAGOM_VERSION '$version' does not fit the versionCode formula in docs/release.md"
    }
    return major.toInt() * 1_000_000 + minor.toInt() * 10_000 + patch.toInt() * 100 + n
}
check(versionCodeOf("1.2.3-beta.4") == 1020304 && versionCodeOf("1.2.3") == 1020399)

val lagomVersion = setting("LAGOM_VERSION")
// Written under build/ so the decoded key never lands in a tracked path.
val uploadKeystore = setting("LAGOM_ANDROID_KEYSTORE")?.let { encoded ->
    layout.buildDirectory.file("signing/upload.jks").get().asFile.apply {
        parentFile.mkdirs()
        writeBytes(Base64.getMimeDecoder().decode(encoded))
    }
}

android {
    namespace = "app.lagommail.android"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = "com.alexkerber.lagom"
        minSdk = 29
        targetSdk = 36
        // Development builds without a tag stay at code 1, below every release.
        versionCode = lagomVersion?.let(::versionCodeOf) ?: 1
        versionName = lagomVersion ?: "0.0.0-dev"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    signingConfigs {
        if (uploadKeystore != null) create("upload") {
            storeFile = uploadKeystore
            storePassword = setting("LAGOM_ANDROID_KEYSTORE_PASSWORD")
            keyAlias = setting("LAGOM_ANDROID_KEY_ALIAS")
            keyPassword = setting("LAGOM_ANDROID_KEY_PASSWORD") ?: storePassword
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Without the upload key the release stays unsigned: Play refuses it and nobody mistakes it for a real build.
            signingConfig = signingConfigs.findByName("upload")
        }
    }
    buildFeatures { compose = true }
    sourceSets["main"].kotlin.srcDir("build/generated/uniffi")
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material3:material3-window-size-class")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.core:core-splashscreen:1.2.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("androidx.work:work-runtime-ktx:2.11.2")
    implementation("net.java.dev.jna:jna:5.18.1@aar")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}

val verifyRustArtifacts = tasks.register("verifyRustArtifacts") {
    doLast {
        check(file("build/generated/uniffi/app/lagommail/core/lagom_ffi.kt").isFile) {
            "Run scripts/build-android.sh from the repository root first."
        }
        listOf("arm64-v8a", "x86_64").forEach { abi ->
            check(file("src/main/jniLibs/$abi/liblagom_ffi.so").isFile) { "Missing Rust library for $abi; run scripts/build-android.sh." }
        }
    }
}
tasks.named("preBuild") { dependsOn(verifyRustArtifacts) }
