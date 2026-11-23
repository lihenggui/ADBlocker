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

package li.mercury.adblocker.feature.apps.impl

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.DEFAULT
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.flow.refreshWhileCollected
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.StatsRepository
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import java.text.Collator
import javax.inject.Inject

/** Which apps the list shows. */
enum class AppFilter { Filtered, All }

data class AppsUiState(
    val loading: Boolean = true,
    val failure: LoadFailure? = null,
    val entries: List<AppEntry> = emptyList(),
    val workInventoryUnavailable: Boolean = false,
    val personalEligibilityUnavailable: Boolean = false,
    /** Today's and the week's counts, by package. */
    val counts: ReadState<Map<String, AppCount>> = ReadState.Pending,
    val filter: AppFilter = AppFilter.Filtered,
    /**
     * Apps changed in this view: they stay in it until another chip is
     * chosen, rather than leave it from under the finger that turned them off.
     */
    val touched: Set<String> = emptySet(),
) {
    /** The apps the chosen chip shows. */
    val shown: List<AppEntry>
        get() = entries.filter {
            it.appKey in touched || when (filter) {
                AppFilter.Filtered -> it.isFiltered
                AppFilter.All -> true
            }
        }

    val filteredCount: Int get() = entries.count { it.isFiltered }
}

@HiltViewModel
class AppsViewModel @Inject constructor(
    private val apps: AppsRepository,
    private val stats: StatsRepository,
    @Dispatcher(DEFAULT) private val defaultDispatcher: CoroutineDispatcher,
    changes: DataChanges,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val state = MutableStateFlow(AppsUiState())
    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val uiState: StateFlow<AppsUiState> = state
        .refreshWhileCollected(merge(changes.changes, reload), ::load)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), AppsUiState())

    /** Whether the user picked a chip; until then the first load picks one. */
    private var filterChosen = false

    /**
     * The switches turned and not yet done by the daemon, by package, with a
     * token for the turn: a list read meanwhile does not have them yet, and
     * must not put them back for a moment.
     */
    private val pending = HashMap<String, Pair<Any, AppEntry>>()

    fun refresh() {
        reload.tryEmit(Unit)
    }

    private suspend fun load() {
        suspendRunCatching("apps") {
            val inventory = apps.inventory()
            val entries = withContext(defaultDispatcher) {
                val collator = Collator.getInstance()
                inventory.apps.sortedWith(compareBy(collator) { it.app.label })
            }
            val counts = readSection { stats.summary().perApp }
            state.update {
                val filter = when {
                    filterChosen -> it.filter
                    entries.none { entry -> entry.isFiltered } || entries.any { entry -> entry.inAllAppsScope } -> AppFilter.All
                    else -> AppFilter.Filtered
                }
                val shown = entries.map { entry ->
                    pending[entry.appKey]?.second?.let { AppEntry(entry.app, it.filter) } ?: entry
                }
                it.copy(
                    loading = false,
                    failure = null,
                    entries = shown,
                    workInventoryUnavailable = inventory.workUnavailable,
                    personalEligibilityUnavailable = inventory.personalUnavailable,
                    counts = counts,
                    filter = filter,
                )
            }
        }.onFailure { e ->
            state.update { it.copy(loading = false, failure = e.toLoadFailure()) }
        }
    }

    fun setFilter(filter: AppFilter) {
        filterChosen = true
        state.update { it.copy(filter = filter, touched = emptySet()) }
    }

    /**
     * Starts or stops filtering an app. The switch follows at once; the list
     * is read again when the daemon has done it. If it could not, the switch
     * goes back to where it was just before, with a message, and the list is
     * read again: a change that failed half way may have changed something.
     */
    fun setFiltered(entry: AppEntry, filtered: Boolean, undoable: Boolean = true) {
        val packageName = entry.appKey
        val before = state.value.entries.firstOrNull { it.appKey == packageName } ?: return
        val included = if (before.inAllAppsScope) !before.excluded else before.isSelected
        if (included == filtered || (filtered && !before.canEnableFiltering)) return
        val choice = before.filter ?: FilteredApp(
            before.packageName,
            FilterLevel.Hosts,
            null,
            profileSerial = before.app.profileSerial,
        )
        val after = AppEntry(before.app, if (filtered) choice else null, before.inAllAppsScope, before.inAllAppsScope && !filtered)
        val turn = Any()
        pending[packageName] = turn to after
        replace(packageName) { after }
        viewModelScope.launch {
            try {
                suspendRunCatching("apps toggle") {
                    apps.setFiltered(packageName, filtered)
                    if (!filtered && before.unavailableReason != null) {
                        messages.show(UiMessage(R.string.feature_apps_selection_removed, listOf(entry.app.label)))
                    } else if (undoable) {
                        val text = when {
                            before.inAllAppsScope && filtered -> R.string.feature_apps_included
                            before.inAllAppsScope -> R.string.feature_apps_excluded_message
                            filtered -> R.string.feature_apps_now_filtering
                            else -> R.string.feature_apps_stopped_filtering
                        }
                        messages.show(
                            UiMessage(text, listOf(entry.app.label), R.string.feature_apps_undo) {
                                setFiltered(before, !filtered, undoable = false)
                            },
                        )
                    }
                }.onFailure {
                    replace(packageName) { before }
                    messages.show(UiMessage(R.string.feature_apps_failed))
                    refresh()
                }
            } finally {
                if (pending[packageName]?.first === turn) pending.remove(packageName)
            }
        }
    }

    private fun replace(packageName: String, change: (AppEntry) -> AppEntry) {
        state.update { s ->
            s.copy(entries = s.entries.map { if (it.appKey == packageName) change(it) else it }, touched = s.touched + packageName)
        }
    }
}
