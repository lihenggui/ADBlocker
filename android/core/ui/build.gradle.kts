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
    alias(libs.plugins.adblocker.android.library.compose)
}

android {
    namespace = "li.mercury.adblocker.core.ui"
}

dependencies {
    implementation(libs.androidx.activity.compose)
    api(projects.core.designsystem)
    api(projects.core.model)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtimeCompose)
    implementation(libs.appiconloader)
    implementation(libs.coil.compose)

    testImplementation(libs.kotlin.test.junit)
    testImplementation(libs.robolectric)
}
