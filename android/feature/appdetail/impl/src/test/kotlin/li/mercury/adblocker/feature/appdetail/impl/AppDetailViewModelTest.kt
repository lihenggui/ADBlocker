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

package li.mercury.adblocker.feature.appdetail.impl

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.StatsRepository
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import org.junit.Rule
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals

@OptIn(ExperimentalCoroutinesApi::class)
class AppDetailViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenIndependentCountAndPinReadFailures_whenDetailsReload_thenAvailableSectionsStayUsable() = runTest {
        val app = InstalledApp("com.example.news", "News", null, isSystem = false)
        val pin = PinnedHost(app.packageName, "news.example", Instant.EPOCH)
        var countsFail = true
        var pinsFail = false
        var pins = listOf(pin)
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = AppEntry(app, FilteredApp(packageName, FilterLevel.Hosts, null))

            override suspend fun pinned(packageName: String?): List<PinnedHost> {
                if (pinsFail) throw AbdException.TimedOut()
                return pins
            }
        }
        val stats = object : StatsRepository {
            override suspend fun summary(): StatsSummary {
                if (countsFail) throw AbdException.TimedOut()
                return StatsSummary.Empty
            }
        }
        val viewModel = AppDetailViewModel(app.packageName, apps, stats, ActivityFilter(), TestUserDataRepository(), unexpected(), main.testDispatcher, DataChanges())
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        assertEquals(app, viewModel.uiState.value.entry?.app)
        assertEquals(null, viewModel.uiState.value.failure)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), viewModel.uiState.value.counts)
        assertEquals(ReadState.Available(listOf(pin)), viewModel.uiState.value.pinned)

        countsFail = false
        pinsFail = true
        viewModel.refresh()
        runCurrent()

        assertEquals(ReadState.Available(null), viewModel.uiState.value.counts)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), viewModel.uiState.value.pinned)

        pinsFail = false
        pins = emptyList()
        viewModel.refresh()
        runCurrent()

        assertEquals(ReadState.Available(emptyList<PinnedHost>()), viewModel.uiState.value.pinned)
    }

    @Test
    fun givenAPausedApp_whenUndoIsCollectedAgain_thenItStillTargetsThatApp() = runTest {
        val target = "com.example.news"
        val pauses = mutableListOf<String>()
        val resumes = mutableListOf<String>()
        val apps = object : AppsRepository by unexpected() {
            override suspend fun pause(packageName: String, minutes: Int) {
                pauses += packageName
            }

            override suspend fun resume(packageName: String) {
                resumes += packageName
            }
        }
        val viewModel = AppDetailViewModel(target, apps, unexpected(), ActivityFilter(), TestUserDataRepository(), unexpected(), main.testDispatcher, DataChanges())
        viewModel.pause()
        runCurrent()
        assertEquals(listOf(target), pauses)

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_appdetail_paused, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)

        assertEquals(R.string.feature_appdetail_undo, message.message.actionLabel)
        requireNotNull(message.message.onAction).invoke()
        runCurrent()
        assertEquals(listOf(target), resumes)
    }

    @Test
    fun givenAnEffectiveLevelChange_whenShown_thenTheDetailsReload() = runTest {
        val changes = DataChanges()
        val app = InstalledApp("com.example.news", "News", AppIconSource("com.example.news", "test"), isSystem = false)
        var currentMode = FilterLevel.Full
        var reads = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String): AppEntry {
                reads++
                return AppEntry(app, FilteredApp(packageName, currentMode, null))
            }

            override suspend fun pinned(packageName: String?) = emptyList<PinnedHost>()
        }
        val viewModel = AppDetailViewModel(app.packageName, apps, unexpected(), ActivityFilter(), TestUserDataRepository(), unexpected(), main.testDispatcher, changes)
        val visible = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(1, reads)
        assertEquals(FilterLevel.Full, viewModel.uiState.value.entry?.filter?.level)

        currentMode = FilterLevel.Hosts
        changes.notifyChanged()
        runCurrent()
        assertEquals(2, reads)
        assertEquals(FilterLevel.Hosts, viewModel.uiState.value.entry?.filter?.level)

        visible.cancel()
        advanceTimeBy(5_000)
        runCurrent()
        changes.notifyChanged()
        runCurrent()
        assertEquals(2, reads)
    }

    @Test
    fun givenAWorkMember_whenOpened_thenItsGroupControlsAndRecordingUseOneTarget() = runTest {
        val member = "37:com.example.news"
        val target = "37:@com.example.suite"
        val app = InstalledApp(
            "@com.example.suite",
            "News",
            null,
            isSystem = false,
            sharedProfileSerial = 37,
            sharedMembers = listOf(SharedAppMember("com.example.news", "News"), SharedAppMember("com.example.other", "Other")),
        )
        val writes = mutableListOf<Pair<String, String>>()
        val pinTargets = mutableListOf<String?>()
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = AppEntry(app, FilteredApp(app.packageName, FilterLevel.Hosts, null, profileSerial = 37))

            override suspend fun pinned(packageName: String?): List<PinnedHost> {
                pinTargets += packageName
                return emptyList()
            }

            override suspend fun setFiltered(packageName: String, filtered: Boolean) {
                writes += packageName to "filtered:$filtered"
            }

            override suspend fun pause(packageName: String, minutes: Int) {
                writes += packageName to "pause:$minutes"
            }

            override suspend fun resume(packageName: String) {
                writes += packageName to "resume"
            }

            override suspend fun forgetPinned(packageName: String, host: String?) {
                writes += packageName to "forget:$host"
            }
        }
        val filter = ActivityFilter()
        val counts = AppCount(BlockedCount(3, 2), BlockedCount(3, 2), List(7) { 0L })
        val stats = object : StatsRepository {
            override suspend fun summary() = StatsSummary.Empty.copy(perApp = mapOf(target to counts))
        }
        val viewModel = AppDetailViewModel(member, apps, stats, filter, TestUserDataRepository(), unexpected(), main.testDispatcher, DataChanges())
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(target, viewModel.uiState.value.entry?.appKey)
        assertEquals(ReadState.Available(counts), viewModel.uiState.value.counts)

        viewModel.showActivity()
        viewModel.setFiltered(false)
        viewModel.pause()
        viewModel.resume()
        viewModel.forget("example.com")
        runCurrent()

        assertEquals(target, filter.packageName.value)
        assertEquals(listOf<String?>(target), pinTargets)
        assertEquals(listOf("filtered:false", "pause:60", "resume", "forget:example.com").map { target to it }, writes)
    }
}
