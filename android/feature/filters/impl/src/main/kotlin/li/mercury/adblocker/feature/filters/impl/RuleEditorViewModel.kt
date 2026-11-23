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

import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.setTextAndPlaceCursorAtEnd
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.text.TextRange
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.shareIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.RuleDraftStore
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.data.repository.checkEnteredUrl
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import timber.log.Timber
import java.util.UUID
import javax.inject.Inject

data class RuleEditorUiState(
    val text: TextFieldState,
    val loading: Boolean = true,
    val failure: LoadFailure? = null,
    val saved: String = "",
    val diagnostics: RuleEditorDiagnostics? = null,
    val saving: Boolean = false,
    val savedStateUnknown: Boolean = false,
    val discardFailed: Boolean = false,
    /** The last edits were saved or discarded: the editor closes. */
    val done: Boolean = false,
    /** What the lists say about the address tested last; null when none is. */
    val urlCheck: RuleCheckResult? = null,
) {
    val currentDiagnostics: RuleEditorDiagnostics? get() = diagnostics?.takeIf { text.text.contentEquals(it.checkedText) }
    val changed: Boolean get() = changed(text.text.toString())

    /** [changed] for [current], a copy of the text already at hand: it can be 16 MiB. */
    fun changed(current: String): Boolean = savedStateUnknown || current != saved
    val canUndo: Boolean get() = text.undoState.canUndo
    val canRedo: Boolean get() = text.undoState.canRedo
}

data class RuleEditorDiagnostics(
    val checkedText: String,
    /** The lines that will not be applied, by line number from 1. Null when the check failed. */
    val issues: Map<Int, RuleIssue>?,
)

/** How long typing must stop before the rules are checked and the draft is written. */
private const val CHECK_DELAY_MS = 600L
private const val DRAFT_ID = "rule_editor_draft_id"

@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class RuleEditorViewModel @Inject internal constructor(
    private val rules: RulesRepository,
    private val drafts: RuleDraftStore,
    private val savedState: SavedStateHandle,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)

    // Saved state contains only the id of the private checkpoint.
    private var draftId = savedState.get<String>(DRAFT_ID)?.let(UUID::fromString) ?: newDraftId()

    private val field = TextFieldState()
    private val state = MutableStateFlow(RuleEditorUiState(field))
    val uiState: StateFlow<RuleEditorUiState> = state.asStateFlow()

    private var lastCheckedText = ""
    private var check: Job? = null
    private var loading: Job? = null
    private var discarding: Job? = null
    private val draftLock = Mutex()
    private var lastDraft: String? = null
    private var draftWriteFailed = false
    private var editingStarted = false

    fun startEditing() {
        // Keep checkpoints active for the navigation entry, including while its composition is away.
        if (editingStarted) return
        editingStarted = true
        // One copy of the text per edit for both readers below.
        val texts = snapshotFlow { field.text.toString() }.shareIn(viewModelScope, SharingStarted.Eagerly, replay = 1)
        viewModelScope.launch {
            state.filter { !it.loading && it.failure == null }
                .combine(texts) { current, text -> draftText(current, text) }
                .distinctUntilChanged()
                // A write replaces a file of up to 16 MiB: once typing pauses,
                // not at every key. What is pending is written by save() and
                // flushDraft(), and the write reads the text as it is then.
                .mapLatest { delay(CHECK_DELAY_MS) }
                .collect { checkpointDraft() }
        }
        viewModelScope.launch {
            texts.collect { text ->
                val current = state.value
                if (!current.loading && current.failure == null && text != lastCheckedText && !(current.done && !current.changed)) {
                    state.update { it.copy(done = false) }
                    scheduleCheck(text)
                }
            }
        }
        load()
    }

    private fun newDraftId(): UUID = UUID.randomUUID().also { savedState[DRAFT_ID] = it.toString() }

    fun load() {
        val current = state.value
        if (loading?.isActive == true || (!current.loading && current.failure == null)) return
        loading = viewModelScope.launch {
            state.update { it.copy(loading = true, failure = null) }
            suspendRunCatching("editor") {
                val saved = rules.userRules()
                lastDraft = drafts.read(draftId)
                val restored = lastDraft ?: saved
                field.setTextAndPlaceCursorAtEnd(restored)
                field.undoState.clearHistory()
                state.update { it.copy(loading = false, saved = saved) }
                scheduleCheck(restored, now = true)
            }.onFailure { e ->
                state.update { it.copy(loading = false, failure = e.toLoadFailure()) }
            }
        }
    }

    /** Writes the draft now, without waiting for typing to pause: the screen is going away. */
    fun flushDraft() {
        val current = state.value
        if (!editingStarted || current.loading || current.failure != null) return
        viewModelScope.launch { checkpointDraft() }
    }

    fun undo() = field.undoState.undo()

    fun redo() = field.undoState.redo()

    /** Saves one snapshot so edits made while the daemon applies it stay in the editor. */
    fun save() {
        val current = state.value
        if (current.loading || current.failure != null || current.saving || (current.done && !current.changed) || discarding?.isActive == true) return
        val submitted = field.text.toString()
        state.update { it.copy(saving = true, done = false) }
        viewModelScope.launch {
            checkpointDraft()
            suspendRunCatching("editor save") {
                rules.save(submitted)
            }.onFailure {
                // A failed reload may follow a successful write. Keep the draft until the saved rules are known.
                val saved = suspendRunCatching { rules.userRules() }.getOrNull()
                state.update { it.copy(saved = saved ?: it.saved, saving = false, savedStateUnknown = saved == null) }
                messages.show(UiMessage(R.string.feature_filters_failed))
                return@launch
            }
            state.update { it.copy(saved = submitted, saving = false, savedStateUnknown = false) }
            checkpointDraft()
            val done = !state.value.changed && !state.value.saving
            messages.show(UiMessage(if (done) R.string.feature_filters_rules_saved else R.string.feature_filters_earlier_rules_saved))
            state.update { it.copy(done = done) }
        }
    }

    fun discard() {
        val current = state.value
        if (current.saving || (current.done && !current.changed) || discarding?.isActive == true) return
        val discarded = field.text.toString()
        discarding = viewModelScope.launch {
            draftLock.withLock {
                val error = suspendRunCatching { drafts.write(draftId, null) }.exceptionOrNull()
                if (error != null) {
                    Timber.w("discard editor draft: %s", error.javaClass.simpleName)
                    state.update { it.copy(discardFailed = true) }
                    return@withLock
                }
                lastDraft = null
                if (field.text.toString() != discarded) return@withLock
                field.edit {
                    replace(0, length, state.value.saved)
                    selection = TextRange.Zero
                }
                field.undoState.clearHistory()
                state.update { it.copy(savedStateUnknown = false, discardFailed = false, done = true) }
            }
        }
    }

    // An undo can match the old rules while the daemon is about to replace them.
    private fun draftText(current: RuleEditorUiState, text: String = field.text.toString()): String? = text.takeIf { current.changed(text) || current.saving }

    private suspend fun checkpointDraft() {
        draftLock.withLock {
            val text = draftText(state.value)
            if (text == lastDraft) return
            suspendRunCatching("editor draft") {
                drafts.write(draftId, text)
                lastDraft = text
                draftWriteFailed = false
            }.onFailure {
                if (text == null) {
                    draftId = newDraftId()
                    lastDraft = null
                    draftWriteFailed = false
                } else {
                    if (!draftWriteFailed) messages.show(UiMessage(R.string.feature_filters_draft_failed))
                    draftWriteFailed = true
                }
            }
        }
    }

    /** What the lists say about a request to [url], from [page], for a resource of [kind]. */
    fun checkUrl(url: String, page: String, kind: String) {
        viewModelScope.launchCatching("editor check", onFailure = {
            messages.show(UiMessage(R.string.feature_filters_failed))
        }) {
            val result = rules.checkEnteredUrl(url, page, kind)
            state.update { it.copy(urlCheck = result) }
        }
    }

    fun clearUrlCheck() = state.update { it.copy(urlCheck = null) }

    private fun scheduleCheck(text: String, now: Boolean = false) {
        lastCheckedText = text
        check?.cancel()
        state.update { it.copy(diagnostics = null) }
        check = viewModelScope.launch {
            if (!now) delay(CHECK_DELAY_MS)
            val issues = suspendRunCatching { rules.check(text) }.getOrNull()?.issues?.associateBy(RuleIssue::line)
            ensureActive()
            if (!field.text.contentEquals(text)) return@launch
            state.update { it.copy(diagnostics = RuleEditorDiagnostics(text, issues)) }
        }
    }
}
