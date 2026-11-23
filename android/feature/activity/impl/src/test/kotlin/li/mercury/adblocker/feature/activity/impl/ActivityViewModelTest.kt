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

package li.mercury.adblocker.feature.activity.impl

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class ActivityViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenAppAndDaemonMetadataChange_whenTheScreenIsObserved_thenLabelsPickerAndIdleStateRefresh() = runTest {
        val personal = InstalledApp("com.example.news", "News", null, isSystem = false)
        val work = InstalledApp("@com.example.shared", "Work group", null, isSystem = false, sharedProfileSerial = 37)
        var entries = listOf(AppEntry(personal, FilteredApp(personal.packageName, FilterLevel.Hosts, pausedUntil = null)), AppEntry(work, null))
        var paused = false
        var appReads = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun apps() = entries.also { appReads++ }
        }
        val daemon = object : DaemonRepository by unexpected() {
            override suspend fun status() = DaemonStatus(
                running = true, pid = null, startedAt = null, version = null, apps = 1, rules = 0, hostRules = null,
                exclusions = null, paused = paused, resumeAt = null, ipv6 = null, listsUpdated = null,
            )
        }
        val changes = DataChanges()
        val recorder = ActivityRecorder(unexpected(), TestUserDataRepository())
        val viewModel = ActivityViewModel(recorder, ActivityFilter(), apps, daemon, changes)
        runCurrent()
        assertEquals(0, appReads)

        val screen = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        val byKey = mapOf(personal.appKey to personal, work.appKey to work)
        assertEquals(ReadState.Available(ActivityApps(byKey, listOf(personal))), viewModel.uiState.value.apps)
        assertEquals(ReadState.Available(false), viewModel.uiState.value.idle)

        entries = listOf(AppEntry(personal, null), AppEntry(work, FilteredApp(work.packageName, FilterLevel.Hosts, pausedUntil = null)))
        paused = true
        changes.notifyChanged()
        runCurrent()
        assertEquals(ReadState.Available(ActivityApps(byKey, listOf(work))), viewModel.uiState.value.apps)
        assertEquals(ReadState.Available(true), viewModel.uiState.value.idle)

        screen.cancel()
        advanceTimeBy(5_001)
        val readsBefore = appReads
        changes.notifyChanged()
        runCurrent()
        assertEquals(readsBefore, appReads)

        paused = false
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(readsBefore + 1, appReads)
        assertEquals(ReadState.Available(false), viewModel.uiState.value.idle)
    }
}
