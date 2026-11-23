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

import androidx.compose.foundation.text.input.setTextAndPlaceCursorAtEnd
import androidx.compose.runtime.snapshots.Snapshot
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.MAX_RULES_BYTES
import li.mercury.adblocker.core.data.repository.RuleDraftStore
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import java.io.IOException
import java.util.UUID
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class RuleEditorViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val drafts = TestRuleDraftStore()

    private val original = "||old.example^\n"
    private val rules = object : RulesRepository by unexpected() {
        var applied = original
        var finishSave: CompletableDeferred<Unit>? = null
        var failAfterWrite = false
        var failRead = false
        var reads = 0
        var inspect: suspend (String) -> RulesCheck = { RulesCheck(0, emptyList()) }
        val submitted = mutableListOf<String>()

        override suspend fun userRules(): String {
            reads++
            if (failRead) throw IOException("could not read applied rules")
            return applied
        }

        override suspend fun save(text: String) {
            submitted += text
            finishSave?.await()
            applied = text
            if (failAfterWrite) throw IOException("could not reload rules")
        }

        override suspend fun check(text: String) = inspect(text)
    }

    private fun TestScope.editor(handle: SavedStateHandle = SavedStateHandle()): RuleEditorViewModel = RuleEditorViewModel(
        rules,
        drafts,
        handle,
    ).also { editor ->
        backgroundScope.launch { editor.uiState.collect {} }
        editor.startEditing()
        runCurrent()
    }

    private fun setText(editor: RuleEditorViewModel, text: String) {
        editor.uiState.value.text.setTextAndPlaceCursorAtEnd(text)
        Snapshot.sendApplyNotifications()
    }

    /** Lets typing pause, as the checkpoint waits for. */
    private fun TestScope.pauseTyping() {
        advanceTimeBy(CHECKPOINT_DELAY_MS)
        runCurrent()
    }

    private fun savedStateOf(handle: SavedStateHandle) = handle.keys().associateWith { handle.get<String>(it) }

    @Test
    fun givenSavedRules_whenTheClosingScreenIsCollectedAgain_thenTheConfirmationIsStillPending() = runTest {
        val viewModel = editor()
        setText(viewModel, "||replacement.example^")
        viewModel.save()
        runCurrent()
        assertTrue(viewModel.uiState.value.done)
        assertTrue(drafts.checkpoints.isEmpty())

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_filters_rules_saved, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)

        viewModel.viewModelScope.cancel()
    }

    @Test
    fun givenAnEditorViewModel_whenTheScreenHasNotStarted_thenRulesAndDraftsAreNotLoaded() = runTest {
        val editor = RuleEditorViewModel(rules, drafts, SavedStateHandle())
        runCurrent()
        assertEquals(0, rules.reads)
        val composition = backgroundScope.launch { editor.uiState.collect {} }
        editor.startEditing()
        runCurrent()
        val edit = "||draft.example^"
        setText(editor, edit)
        composition.cancel()
        pauseTyping()
        assertEquals(edit, drafts.checkpoints.values.single())

        backgroundScope.launch { editor.uiState.collect {} }
        editor.startEditing()
        runCurrent()
        assertEquals(edit, editor.uiState.value.text.text)
        assertEquals(1, rules.reads)
        editor.viewModelScope.cancel()
    }

    @Test
    fun givenCheckedRules_whenValidationOfAnEditFails_thenOldIssuesDoNotAppearOnTheEdit() = runTest {
        val issue = RuleIssue(1, original, RuleIssueKind.Invalid)
        val checked = mutableListOf<String>()
        rules.inspect = { text ->
            checked += text
            if (text == original) RulesCheck(0, listOf(issue)) else throw IOException("check failed")
        }
        val editor = editor()
        assertEquals(issue, editor.uiState.value.currentDiagnostics?.issues?.get(1))

        val edit = "||replacement.example^"
        setText(editor, edit)
        assertNull(editor.uiState.value.currentDiagnostics)
        runCurrent()
        advanceTimeBy(599)
        assertEquals(listOf(original), checked)
        advanceTimeBy(1)
        runCurrent()

        assertEquals(listOf(original, edit), checked)
        assertEquals(RuleEditorDiagnostics(edit, null), editor.uiState.value.currentDiagnostics)

        editor.undo()
        Snapshot.sendApplyNotifications()
        runCurrent()
        advanceTimeBy(600)
        runCurrent()
        assertEquals(issue, editor.uiState.value.currentDiagnostics?.issues?.get(1))
        editor.viewModelScope.cancel()
    }

    @Test
    fun savingOneRevisionKeepsNewEditsAndClosesOnlyAfterTheyAreSaved() = runTest {
        val editor = editor()
        rules.finishSave = CompletableDeferred()
        setText(editor, "||first.example^")
        editor.save()
        runCurrent()
        assertTrue(editor.uiState.value.saving)
        assertEquals(listOf("||first.example^"), rules.submitted)

        val newer = "||newer.example^"
        setText(editor, newer)
        runCurrent()
        rules.finishSave!!.complete(Unit)
        runCurrent()
        assertEquals("||first.example^", editor.uiState.value.saved)
        assertEquals(newer, editor.uiState.value.text.text)
        assertTrue(editor.uiState.value.changed)
        assertFalse(editor.uiState.value.done)
        assertEquals(newer, drafts.checkpoints.values.singleOrNull())

        editor.save()
        runCurrent()
        assertEquals(newer, rules.applied)
        assertTrue(editor.uiState.value.done)
        assertFalse(editor.uiState.value.changed)
        assertTrue(drafts.checkpoints.isEmpty())
        editor.viewModelScope.cancel()
    }

    @Test
    fun undoToThePreviousRulesWhileSavingSurvivesRecreation() = runTest {
        val handle = SavedStateHandle()
        val editor = editor(handle)
        val replacement = "||replacement.example^\n"
        rules.finishSave = CompletableDeferred()
        setText(editor, replacement)
        editor.save()
        runCurrent()

        editor.undo()
        Snapshot.sendApplyNotifications()
        pauseTyping()
        assertEquals(original, editor.uiState.value.text.text)
        assertEquals(original, drafts.checkpoints.values.singleOrNull())

        rules.finishSave!!.complete(Unit)
        runCurrent()
        assertTrue(editor.uiState.value.changed)
        assertFalse(editor.uiState.value.done)
        editor.viewModelScope.cancel()

        val restored = editor(SavedStateHandle(savedStateOf(handle)))
        assertEquals(replacement, restored.uiState.value.saved)
        assertEquals(original, restored.uiState.value.text.text)
        assertTrue(restored.uiState.value.changed)
        restored.viewModelScope.cancel()
    }

    @Test
    fun aSaveThatFailsAfterWritingKeepsTheEditAndReadsTheAppliedRules() = runTest {
        val editor = editor()
        rules.finishSave = CompletableDeferred()
        rules.failAfterWrite = true
        val replacement = "||replacement.example^\n"
        setText(editor, replacement)
        editor.save()
        runCurrent()
        editor.undo()
        Snapshot.sendApplyNotifications()
        pauseTyping()

        rules.finishSave!!.complete(Unit)
        runCurrent()
        assertEquals(replacement, editor.uiState.value.saved)
        assertEquals(original, editor.uiState.value.text.text)
        assertTrue(editor.uiState.value.changed)
        assertFalse(editor.uiState.value.done)
        assertEquals(original, drafts.checkpoints.values.singleOrNull())
        editor.viewModelScope.cancel()
    }

    @Test
    fun aFailedSaveWithUnknownResultLeavesTheDraftForAnExplicitDecision() = runTest {
        val editor = editor()
        rules.finishSave = CompletableDeferred()
        rules.failAfterWrite = true
        setText(editor, "||replacement.example^\n")
        editor.save()
        runCurrent()
        editor.undo()
        Snapshot.sendApplyNotifications()
        pauseTyping()

        rules.failRead = true
        rules.finishSave!!.complete(Unit)
        runCurrent()
        assertEquals(original, editor.uiState.value.text.text)
        assertTrue(editor.uiState.value.changed)
        assertFalse(editor.uiState.value.done)
        assertEquals(original, drafts.checkpoints.values.singleOrNull())
        editor.viewModelScope.cancel()
    }

    @Test
    fun givenAnEditorIsTypedIn_whenKeysComeFasterThanThePause_thenTheDraftIsWrittenOnceAfterTheLastOne() = runTest {
        val editor = editor()
        drafts.writes.clear()

        for (text in listOf("||a", "||ab", "||abc.example^")) {
            setText(editor, text)
            advanceTimeBy(CHECKPOINT_DELAY_MS - 1)
            runCurrent()
        }
        assertEquals(emptyList(), drafts.writes)

        advanceTimeBy(1)
        runCurrent()
        assertEquals(listOf<String?>("||abc.example^"), drafts.writes)
        editor.viewModelScope.cancel()
    }

    @Test
    fun givenAnEditIsPending_whenTheScreenFlushesTheDraft_thenItIsWrittenAtOnce() = runTest {
        val editor = editor()
        setText(editor, "||pending.example^")
        runCurrent()
        assertEquals(emptyList(), drafts.writes)

        editor.flushDraft()
        runCurrent()
        assertEquals("||pending.example^", drafts.checkpoints.values.single())

        pauseTyping()
        assertEquals(1, drafts.writes.size)
        editor.viewModelScope.cancel()
    }

    @Test
    fun givenAnEditIsPending_whenItIsSaved_thenTheDraftIsWrittenBeforeTheRules() = runTest {
        val editor = editor()
        rules.finishSave = CompletableDeferred()
        setText(editor, "||pending.example^")
        runCurrent()

        editor.save()
        runCurrent()
        assertEquals("||pending.example^", drafts.checkpoints.values.single())
        assertEquals(listOf("||pending.example^"), rules.submitted)
        editor.viewModelScope.cancel()
    }

    @Test
    fun aLargeDraftRestoresOnlyForItsSavedEntryAndDiscardRemovesIt() = runTest {
        val handle = SavedStateHandle()
        val editor = editor(handle)
        val large = "x".repeat(MAX_RULES_BYTES)
        setText(editor, large)
        pauseTyping()
        assertEquals(MAX_RULES_BYTES, drafts.checkpoints.values.singleOrNull()?.length)
        editor.viewModelScope.cancel()

        val savedState = savedStateOf(handle)
        val id = UUID.fromString(savedState.getValue("rule_editor_draft_id"))
        assertEquals(setOf("rule_editor_draft_id"), savedState.keys)
        assertTrue(large == drafts.checkpoints[id])
        val other = editor()
        assertEquals(original, other.uiState.value.text.text)
        other.viewModelScope.cancel()

        val restored = editor(SavedStateHandle(savedState))
        assertTrue(large == restored.uiState.value.text.text)
        assertTrue(restored.uiState.value.changed)
        restored.discard()
        runCurrent()
        assertTrue(restored.uiState.value.done)
        assertFalse(restored.uiState.value.changed)
        assertTrue(drafts.checkpoints.isEmpty())
        restored.viewModelScope.cancel()

        val reopened = editor(SavedStateHandle(savedState))
        assertEquals(original, reopened.uiState.value.text.text)
        assertFalse(reopened.uiState.value.changed)
        reopened.viewModelScope.cancel()
        assertNull(rules.submitted.lastOrNull())
    }

    @Test
    fun aRuleSetAtTheByteLimitSavesWithoutChangingItsText() = runTest {
        val editor = editor()
        val large = "x".repeat(MAX_RULES_BYTES)
        setText(editor, large)
        editor.save()
        runCurrent()
        assertTrue(large == rules.applied)
        assertTrue(editor.uiState.value.done)
        assertTrue(drafts.checkpoints.isEmpty())
        editor.viewModelScope.cancel()
    }

    @Test
    fun aFailedDiscardKeepsTheEditsAndCanBeRetried() = runTest {
        val editor = editor()
        val text = "||draft.example^"
        setText(editor, text)
        pauseTyping()
        drafts.failRemoval = true

        editor.discard()
        runCurrent()
        assertEquals(text, editor.uiState.value.text.text)
        assertTrue(editor.uiState.value.changed)
        assertTrue(editor.uiState.value.discardFailed)
        assertFalse(editor.uiState.value.done)

        drafts.failRemoval = false
        editor.discard()
        runCurrent()
        assertTrue(editor.uiState.value.done)
        assertFalse(editor.uiState.value.discardFailed)
        assertTrue(drafts.checkpoints.isEmpty())
        editor.viewModelScope.cancel()
    }

    @Test
    fun aSuccessfulSaveAbandonsAnOldDraftThatCannotBeRemoved() = runTest {
        val handle = SavedStateHandle()
        val editor = editor(handle)
        val text = "||draft.example^"
        rules.finishSave = CompletableDeferred()
        setText(editor, text)
        editor.save()
        runCurrent()
        val previousState = savedStateOf(handle)
        drafts.failRemoval = true

        rules.finishSave!!.complete(Unit)
        runCurrent()
        assertTrue(editor.uiState.value.done)
        assertFalse(previousState == savedStateOf(handle))
        editor.viewModelScope.cancel()

        assertEquals(text, drafts.checkpoints.values.single())
        drafts.checkpoints[UUID.fromString(previousState.values.single())] = "||obsolete.example^"
        val restored = editor(SavedStateHandle(savedStateOf(handle)))
        assertEquals(text, restored.uiState.value.text.text)
        assertFalse(restored.uiState.value.changed)
        restored.viewModelScope.cancel()
    }
}

/** The most the view model waits after the last edit before writing the draft. */
private const val CHECKPOINT_DELAY_MS = 600L

internal class TestRuleDraftStore : RuleDraftStore {
    val checkpoints = mutableMapOf<UUID, String>()
    val writes = mutableListOf<String?>()
    var failRemoval = false

    override suspend fun read(id: UUID) = checkpoints[id]

    override suspend fun write(id: UUID, text: String?) {
        writes += text
        if (text == null) {
            if (failRemoval) throw IOException("could not remove checkpoint")
            checkpoints.remove(id)
        } else {
            checkpoints[id] = text
        }
    }
}
