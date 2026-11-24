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

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class AutoStartTest {
    private val ownRootMode = UserData(filterMethod = FilterMethod.Root, onboardingDone = true, moduleInstalledByApp = true)
    private var asked = 0
    private var answer: () -> Boolean = { true }

    private fun autoStart(settings: UserData) = AutoStart(
        TestUserDataRepository(settings),
        object : ModuleRepository by unexpected() {
            override suspend fun startUnasked(): Boolean {
                asked++
                return answer()
            }
        },
    )

    @Test
    fun givenRootModeWithTheAppsOwnModule_whenTheDeviceBoots_thenTheDaemonIsStartedOnce() = runTest {
        assertEquals(AutoStart.Outcome.Started, autoStart(ownRootMode).afterBoot())
        assertEquals(1, asked)
    }

    @Test
    fun givenADaemonThatRunsAlready_whenTheDeviceBoots_thenNothingIsStarted() = runTest {
        answer = { false }
        assertEquals(AutoStart.Outcome.NothingToStart, autoStart(ownRootMode).afterBoot())
        assertEquals(1, asked)
    }

    @Test
    fun givenNothingOfTheAppsToStart_whenTheDeviceBoots_thenRootIsNotAskedFor() = runTest {
        val others = listOf(
            ownRootMode.copy(filterMethod = FilterMethod.Vpn),
            // The VPN is in use until the switch to root mode is finished.
            ownRootMode.copy(filterMethod = FilterMethod.Vpn, rootSwitchPending = true),
            // Root is the method before one is chosen.
            ownRootMode.copy(filterMethod = null, onboardingDone = false),
            ownRootMode.copy(onboardingDone = false),
            // A module in a root manager, which starts it.
            ownRootMode.copy(moduleInstalledByApp = false),
        )
        for (settings in others) {
            assertEquals(AutoStart.Outcome.NotAsked, autoStart(settings).afterBoot(), settings.toString())
        }
        assertEquals(0, asked)
    }

    @Test
    fun givenRootMode_whenTheAppIsOpenedAgainAndAgain_thenTheDaemonIsStartedOnceForTheProcess() = runTest {
        // Whose the module is, is asked of root while the app is open.
        val autoStart = autoStart(ownRootMode.copy(moduleInstalledByApp = false))
        assertEquals(AutoStart.Outcome.Started, autoStart.whenOpened())
        assertEquals(AutoStart.Outcome.NotAsked, autoStart.whenOpened())
        assertEquals(1, asked)
    }

    @Test
    fun givenAStartThatFailed_whenTheAppIsOpenedAgain_thenItIsLeftToTheUser() = runTest {
        answer = { throw AbdException.TimedOut() }
        val autoStart = autoStart(ownRootMode)
        assertEquals(AutoStart.Outcome.Failed, autoStart.whenOpened())
        assertEquals(AutoStart.Outcome.NotAsked, autoStart.whenOpened())
        assertEquals(1, asked)
    }

    @Test
    fun givenAnotherMethodOrNoSetup_whenTheAppIsOpened_thenRootIsNotAskedFor() = runTest {
        for (settings in listOf(ownRootMode.copy(filterMethod = FilterMethod.Vpn), ownRootMode.copy(onboardingDone = false))) {
            assertEquals(AutoStart.Outcome.NotAsked, autoStart(settings).whenOpened(), settings.toString())
        }
        assertEquals(0, asked)
    }

    @Test
    fun givenSetupFinishesWhileTheAppIsOpen_whenItIsOpenedNext_thenTheDaemonIsStarted() = runTest {
        val settings = TestUserDataRepository(ownRootMode.copy(onboardingDone = false))
        val autoStart = AutoStart(
            settings,
            object : ModuleRepository by unexpected() {
                override suspend fun startUnasked() = true
            },
        )
        assertEquals(AutoStart.Outcome.NotAsked, autoStart.whenOpened())
        settings.setOnboardingDone(true)
        assertEquals(AutoStart.Outcome.Started, autoStart.whenOpened())
    }

    @Test
    fun givenRootIsRefused_whenTheDeviceBoots_thenItFailsQuietlyAndIsNotTriedAgain() = runTest {
        answer = { throw AbdException.NoRoot() }
        assertEquals(AutoStart.Outcome.Failed, autoStart(ownRootMode).afterBoot())
        answer = { throw AbdException.TimedOut() }
        assertEquals(AutoStart.Outcome.Failed, autoStart(ownRootMode).afterBoot())
        assertEquals(2, asked)
    }

    @Test
    fun givenTheStartIsCancelled_whenTheDeviceBoots_thenTheCancellationIsNotSwallowed() = runTest {
        answer = { throw CancellationException("stopped") }
        assertFailsWith<CancellationException> { autoStart(ownRootMode).afterBoot() }
    }
}
