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

package li.mercury.adblocker.core.data.apps

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.InstalledApp
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class WorkInventoryTest {
    private val app = InstalledApp("com.example.work", "Work", AppIconSource("com.example.work", "test"), isSystem = false)
    private val healthy = WorkProfile(37, setOf(app.packageName))
    private val empty = WorkProfile(38, emptySet())
    private val unavailable = WorkProfile(39, null)

    @Test
    fun givenOtherProfilesFail_whenReadingWorkApps_thenHealthyProfilesRemain() = runTest {
        val denied = WorkProfile(40, setOf("com.example.denied"))
        val hidden = WorkProfile(41, setOf("com.example.hidden"))
        val inventory = collectWorkApps(
            profiles = listOf(healthy, empty, unavailable, denied, hidden),
            profileUsers = { listOf(40L, 38L, 39L, 37L) },
            serialOf = { it },
            installed = { _, profile, _ ->
                when (profile.serial) {
                    37L -> listOf(app)
                    40L -> throw SecurityException("profile became unavailable")
                    else -> error("no package data to resolve")
                }
            },
        )

        assertEquals(listOf(app), inventory.apps)
        assertTrue(inventory.workUnavailable)
    }

    @Test
    fun givenAnEmptyProfile_whenLauncherCannotReadIt_thenInventoryRemainsComplete() = runTest {
        val inventory = collectWorkApps(
            profiles = listOf(empty),
            profileUsers = { error("no packages to query") },
            serialOf = { _: Long -> error("no users to inspect") },
            installed = { _, _, _ -> error("no packages to resolve") },
        )

        assertEquals(emptyList(), inventory.apps)
        assertFalse(inventory.workUnavailable)
    }

    @Test
    fun givenAPendingAndroidLookup_whenCancelled_thenCancellationPropagates() = runTest {
        assertFailsWith<CancellationException> {
            collectWorkApps(
                profiles = listOf(healthy),
                profileUsers = { listOf(37L) },
                serialOf = { it },
                installed = { _, _, _ -> throw CancellationException("profile lookup cancelled") },
            )
        }
    }
}
