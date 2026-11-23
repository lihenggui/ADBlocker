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
import dagger.assisted.Assisted
import dagger.assisted.AssistedFactory
import dagger.assisted.AssistedInject
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.stateIn
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.feature.activity.api.ActivityFilter

/** One app's recorded requests, beside its page. */
data class AppRequestsUiState(
    val app: InstalledApp? = null,
    val events: List<RequestEvent> = emptyList(),
    val recording: RecordingStatus = RecordingStatus.Idle,
)

/**
 * The requests of one app in the current recording, for the pane beside
 * its page on extra large windows.
 */
@HiltViewModel(assistedFactory = AppRequestsViewModel.Factory::class)
class AppRequestsViewModel @AssistedInject constructor(
    @Assisted private val packageName: String,
    recorder: ActivityRecorder,
    private val apps: AppsRepository,
    private val filter: ActivityFilter,
) : ViewModel() {
    private val app = flow { emit(suspendRunCatching { apps.app(packageName)?.app }.getOrNull()) }

    val uiState: StateFlow<AppRequestsUiState> = combine(app.onStart { emit(null) }, recorder.state) { app, recording ->
        val target = app?.appKey ?: packageName
        AppRequestsUiState(app = app, events = recording.feed.events.filter { it.appKey == target }, recording = recording.status)
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), AppRequestsUiState())

    /** Has the Activity page show this app's requests, before going there. */
    fun showInActivity() {
        filter.packageName.value = uiState.value.app?.appKey ?: packageName
    }

    @AssistedFactory
    interface Factory {
        fun create(packageName: String): AppRequestsViewModel
    }
}
