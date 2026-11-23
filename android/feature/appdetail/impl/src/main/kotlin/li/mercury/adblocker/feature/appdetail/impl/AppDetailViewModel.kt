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

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.assisted.Assisted
import dagger.assisted.AssistedFactory
import dagger.assisted.AssistedInject
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.flow.refreshWhileCollected
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.StatsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import li.mercury.adblocker.feature.activity.api.ActivityFilter

data class AppDetailUiState(
    val loading: Boolean = true,
    val failure: LoadFailure? = null,
    /** Null once loaded if the app is not installed. */
    val entry: AppEntry? = null,
    val counts: ReadState<AppCount?> = ReadState.Pending,
    /**
     * The hosts this app does not accept our certificate for. Read with the
     * rest and kept by this screen only: they are never stored by the app.
     */
    val pinned: ReadState<List<PinnedHost>> = ReadState.Pending,
    val pinnedExpanded: Boolean = false,
    val method: FilterMethod = FilterMethod.Root,
    /** Without root: whether the certificate is in the user store, which full filtering needs. */
    val certificateInstalled: Boolean = false,
)

/** How long "pause this app" lasts. */
internal const val APP_PAUSE_MINUTES = 60

@HiltViewModel(assistedFactory = AppDetailViewModel.Factory::class)
class AppDetailViewModel @AssistedInject constructor(
    @Assisted private val packageName: String,
    private val apps: AppsRepository,
    private val stats: StatsRepository,
    private val activityFilter: ActivityFilter,
    private val userData: UserDataRepository,
    private val https: HttpsRepository,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
    changes: DataChanges,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val state = MutableStateFlow(AppDetailUiState())
    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val uiState: StateFlow<AppDetailUiState> = state
        .refreshWhileCollected(merge(changes.changes, reload), ::load)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), AppDetailUiState())

    fun refresh() {
        reload.tryEmit(Unit)
    }

    private suspend fun load() {
        suspendRunCatching("app detail") {
            val entry = apps.app(packageName)
            val target = entry?.appKey ?: packageName
            val counts = readSection { stats.summary().perApp[target] }
            val pinned = if (entry?.isFiltered == true) {
                readSection { apps.pinned(target) }
            } else {
                ReadState.Available(emptyList())
            }
            val capabilities = userData.userData.first().capabilities
            val installed = entry?.isFiltered == true && capabilities.needsUserCa && withContext(io) { https.certificateInstalled() }
            state.update {
                it.copy(
                    loading = false,
                    failure = null,
                    entry = entry,
                    counts = counts,
                    pinned = pinned,
                    method = capabilities.method,
                    certificateInstalled = installed,
                )
            }
        }.onFailure { e ->
            state.update { it.copy(loading = false, failure = e.toLoadFailure()) }
        }
    }

    fun setFiltered(filtered: Boolean) = act {
        val target = target()
        if (!filtered || apps.app(target)?.canEnableFiltering == true) apps.setFiltered(target, filtered)
    }

    fun pause() = act {
        val target = target()
        apps.pause(target, APP_PAUSE_MINUTES)
        val text = if (state.value.entry?.app?.isSharedGroup == true) R.string.feature_appdetail_group_paused else R.string.feature_appdetail_paused
        messages.show(UiMessage(text, actionLabel = R.string.feature_appdetail_undo) { act { apps.resume(target) } })
    }

    fun resume() = act { apps.resume(target()) }

    fun togglePinned() = state.update { it.copy(pinnedExpanded = !it.pinnedExpanded) }

    fun forget(host: String?) = act {
        apps.forgetPinned(target(), host)
        messages.show(UiMessage(R.string.feature_appdetail_forgotten))
    }

    /** Has the Activity page show this app's requests only. */
    fun showActivity() {
        activityFilter.packageName.value = target()
    }

    private fun target(): String = state.value.entry?.appKey ?: packageName

    private fun act(block: suspend () -> Unit) {
        viewModelScope.launchCatching("app detail action", onFailure = {
            messages.show(UiMessage(R.string.feature_appdetail_failed))
        }) {
            block()
        }
    }

    @AssistedFactory
    interface Factory {
        fun create(packageName: String): AppDetailViewModel
    }
}
