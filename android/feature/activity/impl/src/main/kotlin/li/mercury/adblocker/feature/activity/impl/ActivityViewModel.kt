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

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.stateIn
import li.mercury.adblocker.core.common.flow.refreshWhileCollected
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import timber.log.Timber
import javax.inject.Inject

/** Which requests the list shows. */
enum class VerdictFilter { All, Blocked, Allowed, Encrypted }

data class ActivityApps(
    /** Names and icons for the recorded requests, indexed by app key. */
    val byKey: Map<String, InstalledApp>,
    /** The filtered apps offered in the app picker. */
    val filtered: List<InstalledApp>,
)

data class ActivityUiState(
    val events: List<RequestEvent> = emptyList(),
    val dropped: Long = 0,
    val verdict: VerdictFilter = VerdictFilter.All,
    val packageName: String? = null,
    val recording: RecordingStatus = RecordingStatus.Idle,
    /** Filtering is paused or stopped, so nothing new can come. */
    val idle: ReadState<Boolean> = ReadState.Pending,
    val failure: LoadFailure? = null,
    val apps: ReadState<ActivityApps> = ReadState.Pending,
    val shown: List<RequestEvent> = events,
)

@HiltViewModel
class ActivityViewModel @Inject constructor(
    private val recorder: ActivityRecorder,
    private val filter: ActivityFilter,
    private val apps: AppsRepository,
    private val daemon: DaemonRepository,
    changes: DataChanges = DataChanges(),
    activityMessages: ActivityMessages = ActivityMessages(),
) : ViewModel() {
    internal val messages = activityMessages.queue
    private val verdict = MutableStateFlow(VerdictFilter.All)
    private val meta = MutableStateFlow(Meta())

    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)

    private data class Meta(
        val apps: ReadState<ActivityApps> = ReadState.Pending,
        val idle: ReadState<Boolean> = ReadState.Pending,
    )

    val uiState: StateFlow<ActivityUiState> = combine(recorder.state, verdict, filter.packageName, meta) { recording, v, app, m ->
        ActivityUiState(
            events = recording.feed.events,
            dropped = recording.feed.dropped,
            verdict = v,
            packageName = app,
            recording = recording.status,
            idle = m.idle,
            failure = recording.failure,
            apps = m.apps,
            shown = if (app == null && v == VerdictFilter.All) {
                recording.feed.events
            } else {
                recording.feed.events.filter {
                    (app == null || it.appKey == app) && when (v) {
                        VerdictFilter.All -> true
                        VerdictFilter.Blocked -> it.blocked
                        VerdictFilter.Allowed -> it.verdict == Verdict.Allowed
                        VerdictFilter.Encrypted -> it.verdict == Verdict.Passed
                    }
                }
            },
        )
    }.refreshWhileCollected(merge(changes.changes, reload), ::loadMeta)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), ActivityUiState())

    /**
     * Reads the apps' names and whether filtering is paused or stopped again;
     * the screen asks each time it is shown, as either may have changed
     * elsewhere meanwhile.
     */
    fun refresh() {
        reload.tryEmit(Unit)
    }

    private suspend fun loadMeta() {
        val appMetadata = readSection {
            val entries = apps.apps()
            ActivityApps(
                byKey = entries.associate { it.appKey to it.app },
                filtered = entries.filter { it.isFiltered }.map { it.app },
            )
        }
        val idle = readSection { daemon.status().let { !it.running || it.paused } }
        meta.value = Meta(appMetadata, idle)
    }

    fun startRecording(launchService: () -> Unit) {
        if (!recorder.requestStart()) return
        try {
            launchService()
        } catch (e: RuntimeException) {
            Timber.w("start recording: %s", e.javaClass.simpleName)
            recorder.startFailed(e.toLoadFailure())
        }
        refresh()
    }

    fun setVerdict(filter: VerdictFilter) {
        verdict.value = filter
    }

    fun setApp(packageName: String?) {
        filter.packageName.value = packageName
    }
}
