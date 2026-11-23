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
}

android {
    namespace = "li.mercury.adblocker.hook"

    defaultConfig {
        consumerProguardFiles("consumer-rules.pro")
    }
}

// The LSPosed module: lets the apps the user enables it for accept the
// local authority where they check certificates themselves.
dependencies {
    // The Xposed API is provided by the framework at run time.
    compileOnly(libs.libxposed.api)
    implementation(libs.hiddenapibypass)

    testImplementation(libs.bouncycastle.bcpkix)
    testImplementation(libs.kotlin.test.junit)
}
