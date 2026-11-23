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

package li.mercury.adblocker.feature.filters.impl

import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.flow.refreshWhileCollected
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.CatalogEntry
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.data.repository.RulesTooLargeException
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.repository.checkEnteredUrl
import li.mercury.adblocker.core.data.repository.hostOf
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import java.text.NumberFormat
import javax.inject.Inject

enum class FiltersTab { Lists, Rules, Trusted }

data class FiltersUiState(
    val loading: Boolean = true,
    val failure: LoadFailure? = null,
    val tab: FiltersTab = FiltersTab.Lists,
    val lists: List<FilterList> = emptyList(),
    /** While an update runs: the state of each list, by address. */
    val updating: Map<String, ListUpdateState>? = null,
    val userRules: String = "",
    val rulesCheck: RulesCheck? = null,
    val rulesCheckUnavailable: Boolean = false,
    val trusted: List<String> = emptyList(),
    val urlCheck: RuleCheckResult? = null,
    /** When the lists are updated by themselves, as the settings say. */
    val schedule: UpdateInterval = UpdateInterval.Daily,
    val wifiOnly: Boolean = true,
) {
    val totalRules: Int get() = lists.filter { it.enabled }.sumOf { it.rules ?: 0 }

    /** Of the update that runs, how much is done, from 0 to 1. */
    val progress: Float
        get() = updating?.takeIf { it.isNotEmpty() }?.let { states ->
            states.values.count { it != ListUpdateState.Fetching } / states.size.toFloat()
        } ?: 0f

    /** The catalog's lists not subscribed to. */
    val catalog: List<CatalogEntry> get() = FilterCatalog.entries.filter { entry -> lists.none { it.url == entry.url } }
}

@HiltViewModel
class FiltersViewModel @Inject constructor(
    private val listsRepository: ListsRepository,
    private val rules: RulesRepository,
    userData: UserDataRepository,
    changes: DataChanges,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val state = MutableStateFlow(FiltersUiState())
    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val uiState: StateFlow<FiltersUiState> = combine(state, userData.userData) { filters, data ->
        filters.copy(schedule = data.updateInterval, wifiOnly = data.updateOnWifiOnly)
    }
        .refreshWhileCollected(merge(changes.changes, reload), ::load)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), FiltersUiState())
    private var update: Job? = null
    private val changingLists = mutableSetOf<String>()
    private var listRevision = 0L

    private var updateAgain = false
    private val pendingDownloads = linkedSetOf<String>()

    fun refresh() {
        reload.tryEmit(Unit)
    }

    private suspend fun load() {
        // The active update reads the lists again as soon as it finishes.
        if (update != null) return
        suspendRunCatching("filters") {
            val revision = listRevision
            val lists = listsRepository.lists()
            state.update { current ->
                if (revision != listRevision) return@update current
                val changes = current.lists.filter { it.url in changingLists }.associateBy { it.url }.toMutableMap()
                val confirmed = lists.map { list ->
                    val choice = changes.remove(list.url)?.enabled
                    if (choice != null) list.copy(enabled = choice) else list
                }
                current.copy(lists = confirmed + changes.values)
            }
            val userRules = rules.userRules()
            val check = suspendRunCatching { rules.check(userRules) }
            val trusted = rules.trustedSites()
            state.update {
                it.copy(
                    loading = false,
                    failure = null,
                    userRules = userRules,
                    rulesCheck = check.getOrNull(),
                    rulesCheckUnavailable = check.isFailure,
                    trusted = trusted,
                )
            }
        }.onFailure { e ->
            state.update { it.copy(loading = false, failure = e.toLoadFailure()) }
        }
    }

    fun selectTab(tab: FiltersTab) = state.update { it.copy(tab = tab) }

    /** Downloads every enabled list when explicitly requested. */
    fun updateLists() = requestUpdate(null)

    private fun requestUpdate(url: String?) {
        if (update != null) {
            if (url == null) updateAgain = true else pendingDownloads.add(url)
            return
        }
        // Started once it is known as the update, so that a second call
        // always sees it, however soon the first ends.
        val job = viewModelScope.launch(start = CoroutineStart.LAZY) {
            state.update { s ->
                s.copy(updating = s.lists.filter { it.enabled && (url == null || it.url == url) }.associate { it.url to ListUpdateState.Fetching })
            }
            // What each list held before, to say how many rules the update added.
            val before = state.value.lists.associate { it.url to (it.rules ?: 0) }
            var added = 0L
            try {
                suspendRunCatching("update") {
                    val events = if (url == null) listsRepository.update() else listsRepository.updateList(url)
                    events.collect { event ->
                        when (event) {
                            is UpdateEvent.Started -> Unit

                            is UpdateEvent.ListProgress -> {
                                if (event.state == ListUpdateState.Updated && event.rules != null) {
                                    added += (event.rules!! - (before[event.url] ?: 0)).coerceAtLeast(0)
                                }
                                state.update { current ->
                                    current.copy(
                                        updating = (current.updating ?: emptyMap()) + (event.url to event.state),
                                        lists = current.lists.map {
                                            val count = event.rules
                                            if (it.url == event.url && it.enabled && count != null) it.copy(rules = count) else it
                                        },
                                    )
                                }
                            }

                            is UpdateEvent.Finished -> {
                                val lists = event.updated + event.unchanged
                                if (url != null && lists + event.failed == 0) return@collect
                                messages.show(
                                    when {
                                        url == null && !event.exclusionsReady -> UiMessage(R.string.feature_filters_exclusions_pending)
                                        url == null && event.exclusionsFailed > 0 -> UiMessage(R.string.feature_filters_exclusions_failed)
                                        event.failed > 0 -> UiMessage(R.plurals.feature_filters_updated_with_failures, listOf(lists, event.failed), quantity = lists)
                                        added > 0 -> UiMessage(R.plurals.feature_filters_updated_added, listOf(lists, NumberFormat.getIntegerInstance().format(added)), quantity = lists)
                                        else -> UiMessage(R.plurals.feature_filters_updated, listOf(lists), quantity = lists)
                                    },
                                )
                            }
                        }
                    }
                }.onFailure {
                    messages.show(UiMessage(R.string.feature_filters_update_failed))
                }
            } finally {
                update = null
                state.update { it.copy(updating = null) }
                startNextUpdate()
            }
        }
        update = job
        // Cancel a read that began before the update took over.
        refresh()
        job.start()
    }

    private fun startNextUpdate() {
        if (updateAgain) {
            updateAgain = false
            updateLists()
        } else {
            val next = pendingDownloads.firstOrNull()
            if (next == null) {
                refresh()
            } else {
                pendingDownloads.remove(next)
                requestUpdate(next)
            }
        }
    }

    fun setEnabled(list: FilterList, enabled: Boolean) {
        val before = state.value.lists.firstOrNull { it.url == list.url } ?: return
        if (before.enabled == enabled || !changingLists.add(list.url)) return
        replaceList(before.copy(enabled = enabled))
        viewModelScope.launch {
            try {
                suspendRunCatching("list selection") {
                    listsRepository.setEnabled(list.url, enabled)
                }.onSuccess {
                    if (enabled) fetchIfMissing(list.url, before.rules)
                }.onFailure {
                    replaceList(before)
                    messages.show(UiMessage(R.string.feature_filters_failed))
                }
            } finally {
                listWriteFinished(list.url)
            }
        }
    }

    private fun replaceList(list: FilterList) {
        state.update { current -> current.copy(lists = current.lists.map { if (it.url == list.url) list else it }) }
    }

    private suspend fun fetchIfMissing(url: String, previousRules: Int?) {
        val saved = suspendRunCatching("list selection readback") {
            listsRepository.lists().firstOrNull { it.url == url }
        }
        val list = saved.getOrNull()
        if (list != null) replaceList(list)
        val needsDownload = if (list != null) list.enabled && list.rules == null else saved.isFailure && previousRules == null
        if (needsDownload) requestUpdate(url)
    }

    private fun listWriteFinished(url: String) {
        listRevision++
        changingLists.remove(url)
        refresh()
    }

    fun subscribe(entry: CatalogEntry) = subscribe(entry.url)

    fun subscribe(url: String) = act {
        val address = url.trim()
        if (!address.startsWith("https://")) {
            messages.show(UiMessage(R.string.feature_filters_https_only))
            return@act
        }
        val existing = state.value.lists.firstOrNull { it.url == address }
        if (existing != null) {
            setEnabled(existing, true)
            return@act
        }
        if (!changingLists.add(address)) return@act
        val list = FilterList(address, enabled = true, title = null, rules = null, updated = null, error = null)
        state.update { it.copy(lists = it.lists + list) }
        try {
            listsRepository.add(address)
            messages.show(UiMessage(R.string.feature_filters_subscribed))
            fetchIfMissing(address, previousRules = null)
        } catch (e: Exception) {
            state.update { it.copy(lists = it.lists.filterNot { item -> item.url == address }) }
            throw e
        } finally {
            listWriteFinished(address)
        }
    }

    fun remove(list: FilterList) = act {
        listsRepository.remove(list.url)
        messages.show(UiMessage(R.string.feature_filters_removed))
    }

    fun blockDomain(input: String) = act {
        val host = hostOf(input)
        if (host == null) {
            messages.show(UiMessage(R.string.feature_filters_not_a_domain))
            return@act
        }
        rules.blockDomain(host)
        messages.show(UiMessage(R.string.feature_filters_blocked_domain, listOf(host)))
    }

    fun trust(input: String) = act {
        val host = hostOf(input)
        if (host == null) {
            messages.show(UiMessage(R.string.feature_filters_not_a_domain))
            return@act
        }
        rules.trust(host)
        messages.show(UiMessage(R.string.feature_filters_trusted, listOf(host), R.string.feature_filters_undo) { untrust(host) })
    }

    fun untrust(host: String) = act { rules.untrust(host) }

    fun checkUrl(url: String, page: String, kind: String) = act {
        state.update { it.copy(urlCheck = rules.checkEnteredUrl(url, page, kind)) }
    }

    fun clearUrlCheck() = state.update { it.copy(urlCheck = null) }

    /** Adds the rules in a file the user picked to their own. */
    fun import(uri: Uri) = act {
        val count = try {
            rules.import(uri)
        } catch (_: RulesTooLargeException) {
            messages.show(UiMessage(R.string.feature_filters_import_too_large))
            return@act
        }
        if (count > 0) messages.show(UiMessage(R.plurals.feature_filters_imported, listOf(count), quantity = count))
    }

    /** Writes the user's rules into a file the user picked. */
    fun export(uri: Uri) = act {
        rules.export(uri)
        messages.show(UiMessage(R.string.feature_filters_exported))
    }

    private fun act(block: suspend () -> Unit) {
        viewModelScope.launchCatching("filters action", onFailure = {
            messages.show(UiMessage(R.string.feature_filters_failed))
        }) {
            block()
        }
    }
}
