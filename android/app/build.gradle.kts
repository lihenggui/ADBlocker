/*
 * Copyright (C) 2026 Mercury Li
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 3 as
 * published by the Free Software Foundation.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

import java.util.Properties

plugins {
    alias(libs.plugins.adblocker.android.application)
    alias(libs.plugins.adblocker.android.application.compose)
    alias(libs.plugins.adblocker.android.hilt)
    alias(libs.plugins.adblocker.flashable.module)
    alias(libs.plugins.baselineProfile)
    alias(libs.plugins.dependencyGuard)
    alias(libs.plugins.roborazzi)
}

// Release signing is configured in keystore.properties (not in version
// control), with the keys storeFile, storePassword, keyAlias and keyPassword.
val keystoreProperties = rootProject.file("keystore.properties")
    .takeIf { it.isFile }
    ?.let { file -> Properties().apply { file.inputStream().use(::load) } }
val signingStore = providers.environmentVariable("ADBLOCKER_SIGNING_STORE_FILE")
    .orNull ?: keystoreProperties?.getProperty("storeFile")
val requireReleaseSigning = providers.environmentVariable("ADBLOCKER_REQUIRE_RELEASE_SIGNING").orNull == "1"
check(!requireReleaseSigning || !signingStore.isNullOrBlank()) {
    "Configure release signing before building the distribution."
}

android {
    namespace = "li.mercury.adblocker"

    signingConfigs {
        if (signingStore != null) {
            create("release") {
                storeFile = rootProject.file(signingStore)
                storePassword = providers.environmentVariable("ADBLOCKER_SIGNING_STORE_PASSWORD")
                    .orNull ?: keystoreProperties?.getProperty("storePassword")
                keyAlias = providers.environmentVariable("ADBLOCKER_SIGNING_KEY_ALIAS")
                    .orNull ?: keystoreProperties?.getProperty("keyAlias")
                keyPassword = providers.environmentVariable("ADBLOCKER_SIGNING_KEY_PASSWORD")
                    .orNull ?: keystoreProperties?.getProperty("keyPassword")
            }
        }
    }

    defaultConfig {
        applicationId = "li.mercury.adblocker"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // The ABIs scripts/build.sh builds the engine and the daemon for.
        // Libraries of others would come only from dependencies, and make an
        // APK a 32-bit device installs without the engine.
        ndk.abiFilters += listOf("arm64-v8a", "x86_64")
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Without a release key the build is signed with the debug key,
            // which is enough to install it but differs between machines.
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    androidResources {
        // Locales the app is translated into; drives the per-app language picker.
        generateLocaleConfig = true
        localeFilters += providers.gradleProperty("appLocales").get().split(',')
    }

    testOptions.unitTests.isIncludeAndroidResources = true

    // The dependency list AGP adds to the signing block is encrypted with
    // a key only Google can read; the dependencies are in release-info/
    // of the source archive instead. F-Droid rejects an APK carrying it.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    // The engine for filtering without root, as scripts/build.sh builds it:
    // out/jni/<abi>/libabd_vpn.so, packaged as lib/<abi>/. A build without
    // it goes on, and the VPN method reports itself unavailable.
    sourceSets.named("main") {
        jniLibs.directories.add(rootDir.resolve("../out/jni").path)
    }

    // Compressed in the APK, and extracted for the device's one ABI when
    // the app is installed. Stored, the libraries of every ABI the APK
    // carries count in full in what is downloaded, and the engine's alone
    // made up more than half of it.
    packaging.jniLibs.useLegacyPackaging = true
}

androidComponents {
    onVariants { variant ->
        // The benchmark fixture seeds settings without installing a module or starting a VPN.
        if (variant.buildType == "benchmarkRelease" || variant.buildType == "nonMinifiedRelease") {
            variant.sources.kotlin?.addStaticSourceDirectory("src/benchmarkSetup/kotlin")
            variant.sources.manifests?.addStaticManifestFile("src/benchmarkSetup/AndroidManifest.xml")
        }
    }
}

baselineProfile {
    automaticGenerationDuringBuild = false
    dexLayoutOptimization = true
}

dependencies {
    baselineProfile(projects.benchmarks)
    implementation(projects.feature.onboarding.impl)
    implementation(projects.feature.overview.api)
    implementation(projects.feature.overview.impl)
    implementation(projects.feature.apps.api)
    implementation(projects.feature.apps.impl)
    implementation(projects.feature.appdetail.api)
    implementation(projects.feature.appdetail.impl)
    implementation(projects.feature.filters.api)
    implementation(projects.feature.filters.impl)
    implementation(projects.feature.activity.api)
    implementation(projects.feature.activity.impl)
    implementation(projects.feature.settings.api)
    implementation(projects.feature.settings.impl)
    implementation(projects.feature.https.api)
    implementation(projects.feature.https.impl)

    implementation(projects.core.data)
    implementation(projects.core.designsystem)
    implementation(projects.core.engine)
    implementation(projects.core.navigation)
    implementation(projects.core.ui)
    // The LSPosed module ships in the same APK.
    implementation(projects.hook)
    implementation(projects.sync.work)
    implementation(projects.vpn)

    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material3.adaptive)
    implementation(libs.androidx.compose.material3.adaptive.navigation3)
    implementation(libs.androidx.compose.material3.adaptive.navigationSuite)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.core.splashscreen)
    implementation(libs.androidx.hilt.lifecycle.viewModelCompose)
    implementation(libs.androidx.hilt.work)
    implementation(libs.androidx.lifecycle.runtimeCompose)
    implementation(libs.androidx.lifecycle.viewModel.navigation3)
    implementation(libs.androidx.navigation3.ui)
    implementation(libs.androidx.navigationEvent)
    implementation(libs.androidx.profileinstaller)
    implementation(libs.androidx.work.runtime)
    implementation(libs.coil)
    implementation(libs.libsu.core)

    // Declares the activity the screenshot tests show their content in; debug builds only.
    debugImplementation(libs.androidx.compose.ui.testManifest)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.kotlin.test.junit)
    androidTestImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.robolectric)
    testImplementation(projects.core.screenshotTesting)
    testImplementation(projects.core.testing)
}

// Every library in the release APK, checked in (app/dependencies/): a new
// one, direct or transitive, comes in with a reviewed change.
dependencyGuard {
    configuration("releaseRuntimeClasspath")
}
