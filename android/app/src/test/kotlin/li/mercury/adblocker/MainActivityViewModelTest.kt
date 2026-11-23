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

package li.mercury.adblocker

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.PendingSetup
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.WorkSelection
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

@OptIn(ExperimentalCoroutinesApi::class)
class MainActivityViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenPendingSetupFails_whenTheActivityRetries_thenOnlyTheSuccessfulAttemptSchedulesAnUpdate() = runTest {
        val selected = setOf("com.example.news")
        val settings = TestUserDataRepository(UserData(filterMethod = FilterMethod.Root, onboardingDone = true, pendingApps = selected))
        val firstAttempt = CompletableDeferred<Unit>()
        var attempts = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun configureDeferred(packages: Set<String>, work: WorkSelection) {
                assertEquals(selected, packages)
                if (++attempts == 1) {
                    firstAttempt.await()
                    throw AbdException.TimedOut()
                }
            }
        }
        val module = object : ModuleRepository by unexpected() {
            override suspend fun state() = ModuleState.Ready
        }
        var updates = 0
        val scheduler = object : ListUpdateScheduler by unexpected() {
            override suspend fun updateSoon() {
                updates++
            }
        }
        val setup = PendingSetup(settings, module, apps, unexpected(), unexpected(), unexpected())
        val viewModel = MainActivityViewModel(settings, setup, scheduler)

        viewModel.applyPendingSetup()
        runCurrent()
        viewModel.applyPendingSetup()
        runCurrent()
        assertEquals(1, attempts)
        assertEquals(0, updates)

        firstAttempt.complete(Unit)
        runCurrent()
        assertEquals(1, attempts)
        assertEquals(selected, settings.userData.value.pendingApps)
        assertEquals(0, updates)

        viewModel.applyPendingSetup()
        runCurrent()
        assertEquals(2, attempts)
        assertEquals(emptySet(), settings.userData.value.pendingApps)
        assertEquals(1, updates)
    }

    @Test
    fun givenUnreadSettings_whenTheActivityStartsCollecting_thenSplashWaitsForTheSavedSettings() = runTest {
        val settings = UserData(themeMode = ThemeMode.Dark, onboardingDone = true)
        val ready = CompletableDeferred<UserData>()
        var reads = 0
        val repository = object : UserDataRepository by unexpected() {
            override val userData = flow {
                reads++
                emit(ready.await())
            }
        }
        val pendingSetup = PendingSetup(repository, unexpected(), unexpected(), unexpected(), unexpected(), unexpected())
        val viewModel = MainActivityViewModel(repository, pendingSetup, unexpected())
        runCurrent()
        assertEquals(0, reads)

        val started = backgroundScope.launch { viewModel.userData.collect {} }
        runCurrent()
        assertNull(viewModel.userData.value)
        assertEquals(1, reads)
        ready.complete(settings)
        runCurrent()
        assertEquals(settings, viewModel.userData.value)

        started.cancel()
        advanceTimeBy(5_001)
        assertEquals(settings, viewModel.userData.value)
        backgroundScope.launch { viewModel.userData.collect {} }
        runCurrent()
        assertEquals(2, reads)
        assertEquals(settings, viewModel.userData.value)
    }
}
