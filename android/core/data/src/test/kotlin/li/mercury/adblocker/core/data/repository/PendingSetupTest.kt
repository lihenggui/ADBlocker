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

import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.WorkSelection
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class PendingSetupTest {
    private val appSelections = mutableListOf<Set<String>>()
    private val configured = mutableListOf<Map<String, Boolean>>()
    private val appliedScopes = mutableListOf<FilteringScope>()
    private val workPolicies = mutableListOf<Boolean>()
    private var listFails = false
    private var timeOut = true

    private fun setup(userData: TestUserDataRepository, state: ModuleState = ModuleState.Ready) = PendingSetup(
        userData = userData,
        module = object : ModuleRepository by unexpected() {
            override suspend fun state() = state
        },
        apps = object : AppsRepository by unexpected() {
            override suspend fun configureDeferred(packages: Set<String>, work: WorkSelection) {
                val preserveWorkApps = work == WorkSelection.Keep
                if ("com.example.retry" in packages && timeOut) throw AbdException.TimedOut()
                appSelections += packages
                workPolicies += preserveWorkApps
            }
        },
        lists = object : ListsRepository by unexpected() {
            override suspend fun configure(choices: Map<String, Boolean>) {
                if (listFails) throw AbdException.TimedOut()
                configured += choices
            }
        },
        methods = unexpected(),
        daemon = object : DaemonRepository by unexpected() {
            override suspend fun setFilteringScope(scope: FilteringScope) {
                appliedScopes += scope
            }
        },
    )

    private val chosen = UserData(
        onboardingDone = true,
        pendingApps = setOf("com.example.news", "com.example.retry", "com.example.video"),
        pendingDisabledLists = setOf("https://lists.example/disabled.txt"),
        pendingEnabledLists = setOf("https://lists.example/regional.txt"),
    )

    @Test
    fun givenDeferredAllAppsWithoutChoices_whenApplied_thenAllAppsScopeIsSet() = runTest {
        val userData = TestUserDataRepository(UserData(onboardingDone = true, pendingFilteringScope = FilteringScope.All))
        assertTrue(setup(userData).applyIfReady())
        assertEquals(listOf(FilteringScope.All), appliedScopes)
        assertEquals(null, userData.userData.value.pendingFilteringScope)
    }

    @Test
    fun givenDeferredSetup_whenApplied_thenTheOriginalWorkPolicyIsUsed() = runTest {
        timeOut = false
        assertTrue(setup(TestUserDataRepository(chosen)).applyIfReady())
        assertTrue(setup(TestUserDataRepository(chosen.copy(pendingPreserveWorkApps = false))).applyIfReady())
        assertEquals(listOf(true, false), workPolicies)
    }

    @Test
    fun failedSubscriptionsAreRetriedTogetherBeforeAnyAppIsEnabled() = runTest {
        val userData = TestUserDataRepository(chosen)
        listFails = true
        assertFalse(setup(userData).applyIfReady())
        assertEquals(chosen, userData.userData.value)
        assertTrue(appSelections.isEmpty())

        listFails = false
        timeOut = false
        assertTrue(setup(userData).applyIfReady())
        assertEquals(listOf(mapOf("https://lists.example/disabled.txt" to false, "https://lists.example/regional.txt" to true)), configured)
        assertEquals(listOf(chosen.pendingApps), appSelections)
        assertFalse(userData.userData.value.hasPendingLists)
    }

    @Test
    fun aLaterLaunchRetriesTheWholeSelectionIfAnAppWriteFails() = runTest {
        val userData = TestUserDataRepository(chosen)
        setup(userData).applyIfReady()
        assertEquals(chosen, userData.userData.value)
        assertTrue(appSelections.isEmpty())

        timeOut = false
        assertTrue(setup(userData).applyIfReady())
        assertEquals(listOf(chosen.pendingApps), appSelections)
        assertEquals(2, configured.size)
        assertEquals(emptySet(), userData.userData.value.pendingApps)
        assertFalse(setup(userData).applyIfReady())
    }

    @Test
    fun nothingIsAppliedBeforeTheModuleRuns() = runTest {
        val userData = TestUserDataRepository(chosen)
        assertFalse(setup(userData, ModuleState.NeedsRestart).applyIfReady())
        assertTrue(appSelections.isEmpty())
        assertEquals(chosen.pendingApps, userData.userData.value.pendingApps)
    }

    @Test
    fun withNothingChosenThereIsNothingToApply() = runTest {
        assertFalse(setup(TestUserDataRepository()).applyIfReady())
    }

    @Test
    fun aRegionalSubscriptionOnItsOwnWaitsForTheModuleAndIsRetriedIfItFails() = runTest {
        val regional = "https://lists.example/regional.txt"
        val userData = TestUserDataRepository(UserData(onboardingDone = true, pendingEnabledLists = setOf(regional)))

        assertFalse(setup(userData, ModuleState.NeedsRestart).applyIfReady())
        listFails = true
        assertFalse(setup(userData).applyIfReady())
        assertEquals(setOf(regional), userData.userData.value.pendingEnabledLists)

        listFails = false
        assertTrue(setup(userData).applyIfReady())
        assertEquals(listOf(mapOf(regional to true)), configured)
        assertEquals(listOf(emptySet()), appSelections)
        assertTrue(userData.userData.value.pendingEnabledLists.isEmpty())
    }

    @Test
    fun choicesForTheModuleAreNotAppliedToTheVpn() = runTest {
        val userData = TestUserDataRepository(chosen.copy(filterMethod = FilterMethod.Vpn))
        assertFalse(setup(userData).applyIfReady())
        assertTrue(appSelections.isEmpty())
        assertEquals(chosen.pendingApps, userData.userData.value.pendingApps)
    }
}
