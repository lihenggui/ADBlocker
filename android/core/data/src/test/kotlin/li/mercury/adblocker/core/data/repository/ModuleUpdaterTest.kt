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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class ModuleUpdaterTest {
    private val rootMode = UserData(filterMethod = FilterMethod.Root, onboardingDone = true)
    private var looks = 0
    private var installs = 0
    private var needed: () -> Boolean = { true }
    private var install: suspend () -> Boolean = { true }
    private val module = object : ModuleRepository by unexpected() {
        override suspend fun needsUpdate(): Boolean {
            looks++
            return needed()
        }

        override suspend fun installBundled(): Boolean {
            installs++
            return install()
        }
    }

    private fun updater(settings: UserData = rootMode) = ModuleUpdater(TestUserDataRepository(settings), module)

    @Test
    fun givenAnOlderModule_whenTheAppIsOpenedAgainAndAgain_thenItIsReplacedOnceForTheProcess() = runTest {
        val updater = updater()
        updater.updateIfOutdated()
        updater.updateIfOutdated()
        assertEquals(1, looks)
        assertEquals(1, installs)
        assertEquals(ModuleUpdate.Idle, updater.state.value)
    }

    @Test
    fun givenAModuleNotToBeReplacedUnasked_whenTheAppIsOpened_thenNothingIsInstalled() = runTest {
        // Up to date, turned off, marked for removal, newer, or waiting for a restart: the repository says.
        needed = { false }
        val updater = updater()
        updater.updateIfOutdated()
        assertEquals(0, installs)
        assertEquals(ModuleUpdate.Idle, updater.state.value)
    }

    @Test
    fun givenAnotherMethodOrNoSetup_whenTheAppIsOpened_thenRootIsNotAskedFor() = runTest {
        for (settings in listOf(rootMode.copy(filterMethod = FilterMethod.Vpn), rootMode.copy(onboardingDone = false))) {
            updater(settings).updateIfOutdated()
        }
        assertEquals(0, looks)
    }

    @Test
    fun givenTheModuleCannotBeLookedAt_whenTheAppIsOpened_thenNothingIsInstalledAndNoFailureIsShown() = runTest {
        needed = { throw AbdException.NoRoot() }
        val updater = updater()
        updater.updateIfOutdated()
        assertEquals(0, installs)
        assertEquals(ModuleUpdate.Idle, updater.state.value)
    }

    @Test
    fun givenAnInstallThatFails_whenAskedAgain_thenItIsTriedAgain() = runTest {
        install = { throw AbdException.TimedOut() }
        val updater = updater()
        updater.updateIfOutdated()
        assertEquals(ModuleUpdate.Failed(LoadFailure.TimedOut), updater.state.value)
        // Not again by itself.
        updater.updateIfOutdated()
        assertEquals(1, installs)

        install = { true }
        updater.update()
        assertEquals(2, installs)
        assertEquals(ModuleUpdate.Idle, updater.state.value)
    }

    @Test
    fun givenAnUpdateOnRequest_whenItRuns_thenTheStateSaysSoUntilItIsDone() = runTest {
        val installed = CompletableDeferred<Boolean>()
        install = { installed.await() }
        val updater = updater()
        val asked = launch { updater.update() }
        runCurrent()
        assertEquals(ModuleUpdate.Updating, updater.state.value)
        installed.complete(true)
        asked.join()
        assertEquals(ModuleUpdate.Idle, updater.state.value)
        // The repository was not asked whether to: the user was.
        assertEquals(0, looks)
    }
}
