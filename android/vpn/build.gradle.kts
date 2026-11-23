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
}

android {
    namespace = "li.mercury.adblocker.vpn"
}

// The VPN of the VPN method: the service that routes the chosen apps into
// the engine (docs/engine-jni.md), and the controller the screens use.
dependencies {
    implementation(projects.core.data)
    implementation(projects.core.engine)

    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)

    testImplementation(libs.kotlin.test.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(projects.core.testing)
}
