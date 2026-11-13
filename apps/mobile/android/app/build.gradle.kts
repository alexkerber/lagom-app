plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Sign-in client IDs come from a Gradle property (for example in ~/.gradle/gradle.properties) or the
// environment, never from the repository. Empty hides that provider's sign-in button.
fun clientId(property: String, variable: String) =
    providers.gradleProperty(property).orElse(providers.environmentVariable(variable)).getOrElse("").trim()
val googleClientId = clientId("lagom.googleClientId", "LAGOM_GOOGLE_ANDROID_CLIENT_ID")
val microsoftClientId = clientId("lagom.microsoftClientId", "LAGOM_MICROSOFT_ANDROID_CLIENT_ID")

android {
    namespace = "app.lagommail.android"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = "com.alexkerber.lagom"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        buildConfigField("String", "GOOGLE_CLIENT_ID", "\"$googleClientId\"")
        buildConfigField("String", "MICROSOFT_CLIENT_ID", "\"$microsoftClientId\"")
        // Google redirects to the reversed client ID. Without one the filter repeats the Microsoft scheme.
        val suffix = ".apps.googleusercontent.com"
        manifestPlaceholders["googleScheme"] =
            if (googleClientId.endsWith(suffix)) "com.googleusercontent.apps.${googleClientId.removeSuffix(suffix)}" else "msauth.$applicationId"
    }
    buildFeatures { compose = true; buildConfig = true }
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
