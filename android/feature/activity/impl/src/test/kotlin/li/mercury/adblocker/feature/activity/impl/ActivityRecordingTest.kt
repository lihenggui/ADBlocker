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
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.ActivityBuffer
import li.mercury.adblocker.core.data.repository.ActivityFeed
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import org.junit.Rule
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class ActivityRecordingTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Passed, "news.example", null, null, null, null)
    private val other = request.copy(id = 2, packageName = "com.example.notes")
    private val updates = MutableSharedFlow<ActivityFeed>(extraBufferCapacity = 1)
    private val opened = mutableListOf<String?>()
    private var closed = 0
    private var fail = false

    private val activity = object : ActivityRepository by unexpected() {
        override fun feed(packageName: String?, buffer: ActivityBuffer?) = flow {
            opened += packageName
            try {
                if (fail) {
                    emit(ActivityFeed(listOf(request), 0))
                    throw AbdException.CommandRejected()
                }
                updates.collect { emit(it) }
            } finally {
                closed++
            }
        }
    }
    private val apps = object : AppsRepository by unexpected() {
        override suspend fun app(packageName: String) = null
    }
    private val userData = TestUserDataRepository(UserData(filterMethod = FilterMethod.Root))
    private val recorder = ActivityRecorder(activity, userData)
    private val filter = ActivityFilter()

    private fun viewModel() = ActivityViewModel(recorder, filter, apps, unexpected())

    @Test
    fun givenMetadataReadsFail_whenRecordingContinues_thenRequestsStayAndMetadataCanRecover() = runTest {
        var appFailure = true
        var statusFailure = true
        val apps = object : AppsRepository by unexpected() {
            override suspend fun apps(): List<AppEntry> {
                if (appFailure) throw AbdException.TimedOut()
                return emptyList()
            }
        }
        val daemon = object : DaemonRepository by unexpected() {
            override suspend fun status(): DaemonStatus {
                if (statusFailure) throw AbdException.TimedOut()
                return DaemonStatus(
                    running = true, pid = null, startedAt = null, version = null, apps = 0, rules = 0, hostRules = null,
                    exclusions = null, paused = false, resumeAt = null, ipv6 = null, listsUpdated = null,
                )
            }
        }
        val viewModel = ActivityViewModel(recorder, filter, apps, daemon)
        backgroundScope.launch { viewModel.uiState.collect {} }
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        updates.emit(ActivityFeed(listOf(request), 0))
        runCurrent()

        val failed = viewModel.uiState.value
        assertEquals(RecordingStatus.Recording, failed.recording)
        assertEquals(listOf(request), failed.shown)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), failed.apps)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), failed.idle)

        appFailure = false
        viewModel.refresh()
        runCurrent()
        assertEquals(ReadState.Available(ActivityApps(emptyMap(), emptyList())), viewModel.uiState.value.apps)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), viewModel.uiState.value.idle)

        statusFailure = false
        viewModel.refresh()
        runCurrent()
        assertEquals(ReadState.Available(false), viewModel.uiState.value.idle)
        assertEquals(listOf(request), viewModel.uiState.value.shown)
    }

    @Test
    fun aMemberPaneShowsRecordedEventsOfItsOwnGroupAndProfile() = runTest {
        val group = InstalledApp("@com.shared", "News group", null, isSystem = false, sharedProfileSerial = 37)
        val members = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = AppEntry(group, null)
        }
        val pane = AppRequestsViewModel("37:${request.packageName}", recorder, members, filter)
        backgroundScope.launch { pane.uiState.collect {} }
        runCurrent()
        assertTrue(opened.isEmpty())
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        val work = request.copy(packageName = group.packageName, profileSerial = 37)
        updates.emit(ActivityFeed(listOf(work, work.copy(profileSerial = null), request), 0))
        runCurrent()
        pane.showInActivity()
        assertEquals(group.appKey, filter.packageName.value)
        assertEquals(listOf(work), pane.uiState.value.events)
    }

    @Test
    fun openingEitherScreenDoesNotStartARecording() = runTest {
        val activity = viewModel()
        val pane = AppRequestsViewModel(request.packageName, recorder, apps, filter)

        backgroundScope.launch { activity.uiState.collect {} }
        backgroundScope.launch { pane.uiState.collect {} }
        runCurrent()

        assertEquals(emptyList(), opened)
        assertEquals(RecordingStatus.Idle, activity.uiState.value.recording)
    }

    @Test
    fun givenARejectedServiceLaunch_whenStartIsRetried_thenOnlyOneLaunchIsPending() = runTest {
        val activity = viewModel()
        var launches = 0

        activity.startRecording {
            launches++
            assertEquals(RecordingStatus.Starting, recorder.state.value.status)
            throw IllegalStateException("service launch rejected")
        }

        assertEquals(RecordingStatus.Stopped, recorder.state.value.status)
        assertEquals(LoadFailure.Failed, recorder.state.value.failure)
        assertTrue(opened.isEmpty())

        activity.startRecording { launches++ }
        activity.startRecording { launches++ }

        assertEquals(2, launches)
        assertEquals(RecordingStatus.Starting, recorder.state.value.status)
        assertEquals(null, recorder.state.value.failure)
        assertTrue(opened.isEmpty())
    }

    @Test
    fun recordingContinuesWithoutThePageAndStoppingKeepsTheResultsForANewViewModel() = runTest {
        val activity = viewModel()
        val visible = backgroundScope.launch { activity.uiState.collect {} }
        assertTrue(recorder.requestStart())
        val capture = backgroundScope.launch { recorder.record() }
        runCurrent()
        assertFalse(recorder.requestStart())
        updates.emit(ActivityFeed(listOf(request), 0))
        runCurrent()
        assertEquals(listOf(request), activity.uiState.value.shown)

        visible.cancel()
        runCurrent()
        updates.emit(ActivityFeed(listOf(other, request), 0))
        runCurrent()
        assertEquals(0, closed)

        capture.cancel()
        runCurrent()
        assertEquals(1, closed)
        val reopened = viewModel()
        backgroundScope.launch { reopened.uiState.collect {} }
        runCurrent()
        assertEquals(RecordingStatus.Stopped, reopened.uiState.value.recording)
        assertEquals(listOf(other, request), reopened.uiState.value.shown)
        assertEquals(listOf<String?>(null), opened)
    }

    @Test
    fun theAppPaneAndFiltersUseTheSameRecording() = runTest {
        val activity = viewModel()
        val pane = AppRequestsViewModel(request.packageName, recorder, apps, filter)
        backgroundScope.launch { activity.uiState.collect {} }
        backgroundScope.launch { pane.uiState.collect {} }
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        updates.emit(ActivityFeed(listOf(other, request), 0))
        runCurrent()

        pane.showInActivity()
        runCurrent()
        assertEquals(listOf(request), pane.uiState.value.events)
        assertEquals(listOf(request), activity.uiState.value.shown)
        activity.setVerdict(VerdictFilter.Blocked)
        runCurrent()
        assertTrue(activity.uiState.value.shown.isEmpty())
        assertEquals(listOf<String?>(null), opened)
        assertEquals(0, closed)
    }

    @Test
    fun givenTheSamePackageInBothProfiles_whenFilteringARecording_thenOnlyThatInstanceIsShown() = runTest {
        val activity = viewModel()
        val work = request.copy(id = 3, profileSerial = 37)
        val pane = AppRequestsViewModel(work.appKey, recorder, apps, filter)
        backgroundScope.launch { activity.uiState.collect {} }
        backgroundScope.launch { pane.uiState.collect {} }
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        updates.emit(ActivityFeed(listOf(work, request), 0))

        pane.showInActivity()
        runCurrent()
        assertEquals(listOf(work), pane.uiState.value.events)
        assertEquals(listOf(work), activity.uiState.value.shown)

        activity.setApp(request.appKey)
        runCurrent()
        assertEquals(listOf(request), activity.uiState.value.shown)
        assertEquals(listOf(work), pane.uiState.value.events)
        assertEquals(listOf<String?>(null), opened)
    }

    @Test
    fun anInterruptedRecordingKeepsResultsUntilANewRecordingStarts() = runTest {
        fail = true
        assertTrue(recorder.requestStart())
        recorder.record()

        assertEquals(LoadFailure.Failed, recorder.state.value.failure)
        assertEquals(listOf(request), recorder.state.value.feed.events)
        assertEquals(RecordingStatus.Stopped, recorder.state.value.status)

        fail = false
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        assertTrue(recorder.state.value.feed.events.isEmpty())
        assertEquals(RecordingStatus.Recording, recorder.state.value.status)
    }

    @Test
    fun givenARecording_whenTheFilterMethodChanges_thenItStopsAndKeepsWhatArrived() = runTest {
        assertTrue(recorder.requestStart())
        backgroundScope.launch { recorder.record() }
        runCurrent()
        updates.emit(ActivityFeed(listOf(request), 0))
        runCurrent()

        userData.userData.update { it.copy(filterMethod = FilterMethod.Vpn) }
        runCurrent()

        assertEquals(RecordingStatus.Stopped, recorder.state.value.status)
        assertEquals(listOf(request), recorder.state.value.feed.events)
        assertEquals(null, recorder.state.value.failure)
        assertEquals(1, closed)
    }
}
