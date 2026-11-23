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

package li.mercury.adblocker.core.data.repository

import android.content.ContextWrapper
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.apps.InstalledAppsSource
import li.mercury.adblocker.core.data.protocol.AppEligibility
import li.mercury.adblocker.core.data.protocol.SharedUidGroup
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.WorkSelection
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class AppsRepositoryTest {
    private val added = mutableListOf<String>()
    private val changes = DataChanges()

    private fun repository(abd: AbdClient, method: FilterMethod = FilterMethod.Root) = DefaultAppsRepository(
        abd = abd,
        // Choosing an app asks nothing of the package manager.
        installed = InstalledAppsSource(ContextWrapper(null), Dispatchers.Unconfined, CoroutineScope(Dispatchers.Unconfined)),
        userData = TestUserDataRepository(UserData(filterMethod = method)),
        changes = changes,
        root = RootTransport(RootShell(Dispatchers.Unconfined)),
    )

    /** A daemon whose `app add` writes down what it was asked, and fails if [fails]. */
    private fun abd(fails: Boolean = false) = object : AbdClient by unexpected() {
        override suspend fun appSelections() = AppSelections()

        override suspend fun addApp(packageName: String) {
            added += packageName
            if (fails) throw AbdException.CommandFailed("app add failed")
        }
    }

    /** How many times the screens were told to read again, from now on. */
    private fun TestScope.notifications(): List<Unit> = mutableListOf<Unit>().also { seen ->
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { changes.changes.collect { seen += it } }
    }

    @Test
    fun givenRootInventoryFailure_whenMergingInventories_thenPersonalAppsRemain() = runTest {
        val personal = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
        val work = InstalledApp("com.example.work", "Work", AppIconSource("com.example.work", "test"), isSystem = false)
        var unavailable = true
        val daemon = object : AbdClient by unexpected() {
            override suspend fun appSelections() = AppSelections()

            override suspend fun workProfiles(): List<WorkProfile> {
                if (unavailable) throw AbdException.TimedOut()
                return listOf(WorkProfile(37, setOf(work.packageName)))
            }
        }

        val failed = withWorkInventory(listOf(personal), daemon) { error("Android work discovery should not run") }
        assertEquals(listOf(personal), failed.apps)
        assertTrue(failed.workUnavailable)

        unavailable = false
        val recovered = withWorkInventory(listOf(personal), daemon) { profiles ->
            assertEquals(setOf(work.packageName), profiles.single().packages)
            AppInventory(listOf(work))
        }
        assertEquals(listOf(personal, work), recovered.apps)
        assertFalse(recovered.workUnavailable)
    }

    @Test
    fun givenSavedOrCurrentInventoryFailure_whenConfiguring_thenWorkSelectionIsPreserved() = runTest {
        val requested = setOf("com.example.news")
        val configured = mutableListOf<Pair<Set<String>, Boolean>>()
        var error: Exception? = AbdException.BadOutput("app profiles")
        var reads = 0
        val daemon = object : AbdClient by unexpected() {
            override suspend fun appSelections() = AppSelections()

            override suspend fun appEligibility() = AppEligibility(mapOf("com.example.news" to null))

            override suspend fun workProfiles(): List<WorkProfile> {
                reads++
                error?.let { throw it }
                return emptyList()
            }

            override suspend fun configureApps(packages: Set<String>, work: WorkSelection) {
                val preserveWorkApps = work == WorkSelection.Keep
                configured += packages to preserveWorkApps
            }
        }

        val root = repository(daemon)
        root.configure(requested)
        error = null
        root.configure(requested, work = WorkSelection.Keep)
        root.configure(requested)
        repository(daemon, FilterMethod.Vpn).configure(requested, work = WorkSelection.Keep)
        error = CancellationException("inventory cancelled")
        assertFailsWith<CancellationException> { root.configure(requested) }

        assertEquals(listOf(requested to true, requested to true, requested to false, requested to false), configured)
        assertEquals(3, reads)
    }

    @Test
    fun givenPartialPersonalEligibility_whenApplied_thenPreviouslyReadRowsStayVisible() {
        val eligible = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
        val shared = eligible.copy(packageName = "com.example.shared")
        val missing = eligible.copy(packageName = "com.example.missing")
        val report = withPersonalEligibility(
            listOf(eligible, shared, missing),
            mapOf(eligible.packageName to null, shared.packageName to AppUnavailableReason.SharedUid),
        )

        assertTrue(report.personalUnavailable)
        assertTrue(report.apps.first().canEnableFiltering)
        assertEquals(AppUnavailableReason.SharedUid, report.apps[1].unavailableReason)
        assertEquals(AppUnavailableReason.Unavailable, report.apps[2].unavailableReason)
    }

    @Test
    fun givenFailedPersonalEligibility_whenConfiguringRoot_thenNoSelectionIsWritten() = runTest {
        val daemon = object : AbdClient by unexpected() {
            override suspend fun appSelections() = AppSelections()

            override suspend fun appEligibility(): AppEligibility = throw AbdException.TimedOut()
        }

        assertFailsWith<AbdException.TimedOut> {
            repository(daemon).configure(setOf("37:com.example.work"), work = WorkSelection.Keep)
        }
    }

    @Test
    fun givenDeferredHealthyAndSharedPicks_whenConfigured_thenOnlyNewProvenSharedPicksAreSkipped() = runTest {
        val healthy = "com.example.healthy"
        val shared = "com.example.shared"
        val savedShared = "com.example.saved"
        val unknown = "com.example.unknown"
        val work = "37:com.example.work"
        val configured = mutableListOf<Set<String>>()
        val daemon = object : AbdClient by unexpected() {
            override suspend fun appSelections() = AppSelections()

            override suspend fun appEligibility() = AppEligibility(
                mapOf(healthy to null, shared to AppUnavailableReason.SharedUid, savedShared to AppUnavailableReason.SharedUid),
            )

            override suspend fun apps() = listOf(FilteredApp(savedShared, FilterLevel.Full, pausedUntil = null, unavailableReason = AppUnavailableReason.SharedUid))

            override suspend fun configureApps(packages: Set<String>, work: WorkSelection) {
                val preserveWorkApps = work == WorkSelection.Keep
                assertTrue(preserveWorkApps)
                configured += packages
            }
        }

        val repository = repository(daemon)
        repository.configureDeferred(setOf(healthy, shared, savedShared, work), work = WorkSelection.Keep)
        repository.configureDeferred(setOf(healthy, shared, unknown), work = WorkSelection.Keep)
        repository.configure(setOf(healthy, shared), work = WorkSelection.Keep)

        assertEquals(listOf(setOf(healthy, savedShared, work), setOf(healthy, unknown), setOf(healthy, shared)), configured)
    }

    @Test
    fun givenMembersOfOneNamedUid_whenInventoryChanges_thenTheSameTargetKeepsItsSelection() {
        val news = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
        val target = "@com.example.suite"
        val saved = FilteredApp(target, FilterLevel.Full, pausedUntil = null)
        val before = SharedUidGroup("com.example.suite", setOf(news.packageName))
        val after = before.copy(packages = before.packages + "com.example.hidden")
        val installed = listOf(news.copy(unavailableReason = AppUnavailableReason.SharedUid))
        val rows = withSharedGroups(installed, listOf(before), mapOf(target to saved), requested = null, profileSerial = null)
        val refreshed = withSharedGroups(installed, listOf(after), mapOf(target to saved), AppIdentity("com.example.hidden"), profileSerial = null)
        assertEquals(target, rows.single().appKey)
        assertEquals(target, refreshed.single().appKey)
        assertEquals(setOf(news.packageName, "com.example.hidden"), refreshed.single().sharedMembers.map { it.packageName }.toSet())
        assertEquals("com.example.hidden", refreshed.single().sharedMembers.first().label)
        assertTrue(refreshed.single().canEnableFiltering)
    }

    @Test
    fun givenTheSameNamedUidInTwoProfiles_whenListed_thenTargetsAndAvailabilityAreSeparate() {
        val group = SharedUidGroup("com.example.suite", setOf("com.example.news"))
        val personal = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
        val profile = 37L
        val own = withSharedGroups(listOf(personal), listOf(group), emptyMap(), requested = null, profileSerial = null).single()
        val work = withSharedGroups(emptyList(), listOf(group), emptyMap(), requested = null, profileSerial = profile).single()
        assertEquals("@com.example.suite", own.appKey)
        assertEquals("37:@com.example.suite", work.appKey)
        assertTrue(own.canEnableFiltering)
        assertTrue(work.canEnableFiltering)
        assertTrue(work.isSystem)
        assertEquals("com.example.news", work.sharedMembers.single().label)
    }

    @Test
    fun givenOnlyDaemonMetadata_whenListingPersonalGroups_thenOneSystemRowIncludesEveryMember() {
        val group = SharedUidGroup("com.example.suite", setOf("com.example.hidden", "com.example.other"))
        val app = withSharedGroups(emptyList(), listOf(group), emptyMap(), requested = null, profileSerial = null).single()
        assertEquals("@com.example.suite", app.appKey)
        assertTrue(app.isSystem)
        assertTrue(app.canEnableFiltering)
        assertEquals(group.packages, app.sharedMembers.map { it.label }.toSet())
    }

    @Test
    fun givenSavedGroupWithoutCurrentInventory_whenListed_thenItsIdentityRemainsButTheGroupCannotBeEnabled() {
        val saved = FilteredApp("@com.example.suite", FilterLevel.Full, pausedUntil = null, profileSerial = 37)
        val row = withSharedGroups(emptyList(), emptyList(), mapOf(saved.appKey to saved), requested = null, profileSerial = 37).single()
        assertEquals(saved.appKey, row.appKey)
        assertFalse(row.canEnableFiltering)
        assertTrue(row.sharedMembers.isEmpty())
        assertTrue(withSharedGroups(emptyList(), emptyList(), emptyMap(), requested = null, profileSerial = 37).isEmpty())
    }

    @Test
    fun anAppIsAddedUsingTheEnginesDefault() = runTest {
        repository(abd()).setFiltered("com.example.news", true)
        assertEquals(listOf("com.example.news"), added)
    }

    @Test
    fun givenASharedGroup_whenEnabled_thenOnlyRootAcceptsItsIdentity() = runTest {
        val group = "@com.example.suite"
        val daemon = object : AbdClient by unexpected() {
            override suspend fun appSelections() = AppSelections()

            override suspend fun addApp(packageName: String) {
                assertEquals(group, packageName)
                added += packageName
            }
        }

        repository(daemon).setFiltered(group, true)
        assertEquals(listOf(group), added)
        assertFailsWith<AbdException.CommandFailed> { repository(daemon, FilterMethod.Vpn).setFiltered(group, true) }
        assertEquals(1, added.size)
    }

    @Test
    fun aChangeThatFailedHalfWayStillHasTheScreensReadAgain() = runTest {
        val seen = notifications()
        assertFailsWith<AbdException.CommandFailed> { repository(abd(fails = true)).setFiltered("com.example.news", true) }
        testScheduler.advanceUntilIdle()
        assertEquals(1, seen.size)
    }

    @Test
    fun aChangeThatWorkedHasTheScreensReadAgain() = runTest {
        val seen = notifications()
        repository(abd()).setFiltered("com.example.news", true)
        testScheduler.advanceUntilIdle()
        assertEquals(1, seen.size)
    }
}
