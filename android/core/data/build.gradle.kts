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

plugins {
    alias(libs.plugins.adblocker.android.library)
    alias(libs.plugins.adblocker.android.hilt)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "li.mercury.adblocker.core.data"
    sourceSets.named("main") {
        resources.srcDirs(rootDir.resolve("../config"), rootDir.resolve("../abd-engine/src/catalog/data"))
    }
}

dependencies {
    api(projects.core.common)
    api(projects.core.model)

    implementation(projects.core.datastore)
    implementation(projects.core.engine)

    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.libsu.core)

    testImplementation(libs.kotlin.test.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.robolectric)
    testImplementation(projects.core.testing)
}

// The tests read the daemon's outputs shared with its own tests.
val protocolCases = rootDir.resolve("../testdata/protocol")
val timeZoneCases = rootDir.resolve("../testdata/regional-time-zone-cases.tsv")
tasks.withType<Test>().configureEach {
    inputs.dir(protocolCases).withPropertyName("protocolCases").withPathSensitivity(PathSensitivity.RELATIVE)
    systemProperty("protocolCases", protocolCases.absolutePath)
    inputs.file(timeZoneCases).withPropertyName("timeZoneCases").withPathSensitivity(PathSensitivity.RELATIVE)
    systemProperty("timeZoneCases", timeZoneCases.absolutePath)
}
