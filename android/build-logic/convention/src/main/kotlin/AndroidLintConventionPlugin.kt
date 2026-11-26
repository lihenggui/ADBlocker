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

import com.android.build.api.dsl.ApplicationExtension
import com.android.build.api.dsl.LibraryExtension
import com.android.build.api.dsl.Lint
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.configure

/*
 * Includes code from Blocker, modified for ADBlocker.
 * Copyright 2022 The Android Open Source Project
 * Upstream license: Apache-2.0. See THIRD_PARTY_NOTICES.md.
 */

/** The app checks its dependencies with the final merged manifest. Libraries check their own sources. */
class AndroidLintConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        with(target) {
            // AGP does not provide lint tasks for standalone test APKs.
            if (pluginManager.hasPlugin("com.android.test")) {
                return
            }
            val structureOnly = providers.gradleProperty("lintStructureOnly").isPresent
            val androidModule = pluginManager.hasPlugin("com.android.application") ||
                pluginManager.hasPlugin("com.android.library")
            when {
                pluginManager.hasPlugin("com.android.application") ->
                    configure<ApplicationExtension> { lint { configure(structureOnly, checkProjectDependencies = true) } }

                pluginManager.hasPlugin("com.android.library") ->
                    configure<LibraryExtension> { lint { configure(structureOnly, checkProjectDependencies = false) } }

                else -> {
                    pluginManager.apply("com.android.lint")
                    configure<Lint> { configure(structureOnly, checkProjectDependencies = true) }
                }
            }
            dependencies.add("lintChecks", project(":lint"))
            if (structureOnly) {
                tasks.register("lintStructure") {
                    group = "verification"
                    description = "Check the structure of production Kotlin sources."
                    dependsOn(if (androidModule) "lintDebug" else "lint")
                }
            }
        }
    }
}

private fun Lint.configure(structureOnly: Boolean, checkProjectDependencies: Boolean) {
    // The structure run selects each module directly, including support modules.
    checkDependencies = checkProjectDependencies && !structureOnly
    if (structureOnly) {
        checkOnly += structureIssues
    }
    error += listOf(
        "StringFormatMatches",
        "StringFormatInvalid",
        "StringFormatCount",
        "MissingQuantity",
        "UnusedQuantity",
        "ImpliedQuantity",
        "MissingTranslation",
        "ExtraTranslation",
    )
    // Dependabot proposes newer versions.
    disable += "GradleDependency"
}

private val structureIssues = listOf("LongFunction", "LongComposeFile", "LongSourceFile")
