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

import android.os.SystemClock
import android.view.KeyEvent
import androidx.activity.ComponentActivity
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextInputSelection
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextRange
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import com.google.android.apps.common.testing.accessibility.framework.AccessibilityCheckResultUtils
import com.google.android.apps.common.testing.accessibility.framework.checks.TouchTargetSizeCheck
import kotlinx.coroutines.cancel
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.designsystem.theme.ADBlockerTheme
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.model.RuleIssueCode
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.model.RuleIssueReason
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.screenshottesting.DefaultTestDevices
import li.mercury.adblocker.core.screenshottesting.DemoApp
import li.mercury.adblocker.core.screenshottesting.DemoClockRule
import li.mercury.adblocker.core.screenshottesting.DemoData
import li.mercury.adblocker.core.screenshottesting.captureForDevice
import li.mercury.adblocker.core.screenshottesting.captureMultiDevice
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.core.ui.LocalSnackbarHostState
import org.hamcrest.Matchers
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class FiltersScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private val issue = RuleIssue(5, "||ads.example^\$popup", RuleIssueKind.Inapplicable, RuleIssueReason(RuleIssueCode.Known.Option, "popup"))
    private val issues = listOf(issue)
    private val userRules = DemoData.userRules.replace("example.com##.promo-banner", issue.text)

    private val state
        get() = FiltersUiState(
            loading = false,
            lists = DemoData.lists,
            userRules = userRules,
            rulesCheck = RulesCheck(rules = 5, issues = issues),
            trusted = DemoData.trusted,
        )

    /**
     * The FAB floats over the list: at the scroll position captured, it
     * covers part of a row's button, which has its full size scrolled clear
     * (the list ends with room for it).
     */
    private val coveredByFab = AccessibilityCheckResultUtils.matchesChecks(Matchers.equalTo(TouchTargetSizeCheck::class.java))

    @Test
    fun givenLists_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Filters", coveredByFab) { Filters(state) }
    }

    @Test
    fun givenLists_whenShownInDarkTheme_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_dark", DefaultTestDevices.PHONE.spec, "Filters", accessibilitySuppressions = coveredByFab, darkMode = true) { Filters(state) }
    }

    @Test
    fun givenHundredsOfThousandsOfRules_whenShownOnACompactPhone_thenTheFullCountFitsOnOneLine() {
        val list = DemoData.lists.first().copy(enabled = true, rules = 916_048)
        capturePhone("FiltersLargeRuleCount") { Filters(state.copy(lists = listOf(list))) }

        val layouts = mutableListOf<TextLayoutResult>()
        composeTestRule.onNodeWithText("916,048").performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        assertEquals(1, layouts.single().lineCount)
        assertFalse(layouts.single().hasVisualOverflow)
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenLists_whenShownInChinese_thenMatchesGolden() {
        val entry = FilterCatalog.entries.single { it.id == "anti_ad" }
        val antiAd = DemoData.lists.first().copy(url = entry.url, title = entry.title, rules = 102_748)
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Filters", accessibilitySuppressions = coveredByFab) {
            Filters(state.copy(lists = listOf(antiAd) + state.lists))
        }
        composeTestRule.onNodeWithText(entry.title).assertIsDisplayed()
    }

    @Test
    fun givenUpdating_whenShown_thenShowsOneProgressForAllLists() {
        val updating = mapOf(
            DemoData.EASYLIST to ListUpdateState.Updated,
            DemoData.ADGUARD_MOBILE to ListUpdateState.Unchanged,
            DemoData.UBLOCK to ListUpdateState.Fetching,
            DemoData.EASYPRIVACY to ListUpdateState.Fetching,
        )
        capturePhone("FiltersUpdating") { Filters(state.copy(updating = updating)) }
    }

    @Test
    fun givenMyRules_whenShown_thenPreviewsThemWithTheTools() {
        capturePhone("FiltersRules") { Filters(state.copy(tab = FiltersTab.Rules)) }
    }

    @Test
    fun givenTrustedSites_whenShown_thenListsThem() {
        capturePhone("FiltersTrusted") { Filters(state.copy(tab = FiltersTab.Trusted)) }
    }

    @Test
    fun givenRulesWithAnIssue_whenEdited_thenMarksTheLine() {
        val text = TextFieldState(userRules).apply {
            edit {
                append("\n||new.example^")
                selection = TextRange.Zero
            }
        }
        val editor = RuleEditorUiState(
            loading = false,
            text = text,
            saved = userRules,
            diagnostics = RuleEditorDiagnostics(text.text.toString(), issues.associateBy { it.line }),
        )
        capturePhone("RuleEditor") {
            RuleEditorScreen(
                state = editor,
                urlCheck = null,
                onUndo = {},
                onRedo = {},
                onSave = {},
                onCheckUrl = { _, _, _ -> },
                onClearUrlCheck = {},
                onBack = {},
                onRetry = {},
            )
        }
    }

    @Test
    fun givenReplacedSelection_whenMixingToolbarAndKeyboardUndoRedo_thenRestoresTextAndSelection() {
        val original = "||ads.example^\n! comment"
        val edited = "||cdn.example^\n! comment"
        val selected = TextRange(2, 5)
        val rules = object : RulesRepository by unexpected() {
            override suspend fun userRules(): String = original
            override suspend fun check(text: String) = RulesCheck(2, emptyList())
        }
        val drafts = TestRuleDraftStore()
        val viewModel = composeTestRule.runOnUiThread {
            RuleEditorViewModel(rules, drafts, SavedStateHandle())
        }
        try {
            showRuleEditor(viewModel)
            val editor = composeTestRule.onNode(hasSetTextAction())
            editor.assertTextEquals(original)
            editor.performClick()
            editor.performTextInputSelection(selected)
            editor.performTextInput("cdn")
            editor.assertTextEquals(edited)
            // Typing is checkpointed once it pauses; leaving the screen writes it at once.
            composeTestRule.runOnIdle { viewModel.flushDraft() }
            composeTestRule.runOnIdle { assertEquals(edited, drafts.checkpoints.values.singleOrNull()) }
            val undo = composeTestRule.activity.getString(R.string.feature_filters_undo)
            val redo = composeTestRule.activity.getString(R.string.feature_filters_redo)
            composeTestRule.onNodeWithContentDescription(undo).performClick()
            editor.assertTextEquals(original)
            composeTestRule.runOnIdle { assertEquals(selected, viewModel.uiState.value.text.selection) }

            editor.assertIsFocused()
            pressCtrlShortcut(KeyEvent.KEYCODE_Y)
            editor.assertTextEquals(edited)
            composeTestRule.runOnIdle { assertEquals(TextRange(5), viewModel.uiState.value.text.selection) }

            pressCtrlShortcut(KeyEvent.KEYCODE_Z)
            editor.assertTextEquals(original)
            composeTestRule.runOnIdle { assertEquals(selected, viewModel.uiState.value.text.selection) }

            composeTestRule.onNodeWithContentDescription(redo).performClick()
            editor.assertTextEquals(edited)
            composeTestRule.runOnIdle { assertEquals(TextRange(5), viewModel.uiState.value.text.selection) }
        } finally {
            viewModel.viewModelScope.cancel()
        }
    }

    @Test
    fun givenTypingAndANewline_whenUndoingAndSaving_thenSavesTheEarlierTypingRun() {
        val original = "||ads.example^\n"
        var submitted: String? = null
        val rules = object : RulesRepository by unexpected() {
            override suspend fun userRules(): String = original
            override suspend fun check(text: String) = RulesCheck(1, emptyList())
            override suspend fun save(text: String) {
                submitted = text
            }
        }
        val drafts = TestRuleDraftStore()
        val viewModel = composeTestRule.runOnUiThread {
            RuleEditorViewModel(rules, drafts, SavedStateHandle())
        }
        try {
            showRuleEditor(viewModel)
            val editor = composeTestRule.onNode(hasSetTextAction())
            editor.performClick()
            editor.performTextInputSelection(TextRange(original.length))
            for (part in listOf("a", "b", "\n", "c")) editor.performTextInput(part)
            editor.assertTextEquals(original + "ab\nc")

            val undo = composeTestRule.activity.getString(R.string.feature_filters_undo)
            val undoButton = composeTestRule.onNodeWithContentDescription(undo)
            undoButton.performClick()
            editor.assertTextEquals(original + "ab\n")
            undoButton.performClick()
            editor.assertTextEquals(original + "ab")

            val save = composeTestRule.activity.getString(R.string.feature_filters_save)
            composeTestRule.onNodeWithContentDescription(save).performClick()
            composeTestRule.runOnIdle {
                assertEquals(original + "ab", submitted)
                assertTrue(viewModel.uiState.value.done)
            }
        } finally {
            viewModel.viewModelScope.cancel()
        }
    }

    private fun pressCtrlShortcut(keyCode: Int) {
        composeTestRule.runOnIdle {
            val time = SystemClock.uptimeMillis()
            val view = composeTestRule.activity.window.decorView
            for (action in listOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP)) {
                // Robolectric's key map returns a letter for Ctrl+letter. Report a
                // control character so the focused field processes it as a shortcut.
                val event = object : KeyEvent(time, time, action, keyCode, 0, META_CTRL_ON) {
                    override fun getUnicodeChar(): Int = 0
                }
                assertTrue(view.dispatchKeyEvent(event))
            }
        }
    }

    private fun showRuleEditor(viewModel: RuleEditorViewModel) {
        composeTestRule.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                CompositionLocalProvider(LocalSnackbarHostState provides remember { SnackbarHostState() }) {
                    RuleEditorScreen(onBack = {}, viewModel = viewModel)
                }
            }
        }
    }

    @Test
    fun givenAUrlTested_whenShown_thenNamesTheRuleAndItsList() {
        val result = RuleCheckResult(blocked = true, rule = "||adnet.example^", source = RuleSource.List(DemoData.EASYLIST))
        capturePhone("FiltersTestUrl") { TestUrlSheet(result, onCheck = { _, _, _ -> }, onDismiss = {}) }
    }

    @Test
    fun givenBlockADomain_whenOpened_thenAsksForItInASheet() {
        capturePhone("FiltersBlockDomain") {
            TextInputSheet(
                title = stringResource(R.string.feature_filters_block_domain),
                text = stringResource(R.string.feature_filters_block_domain_text),
                label = stringResource(R.string.feature_filters_domain_label),
                confirm = stringResource(R.string.feature_filters_block),
                onConfirm = {},
                onDismiss = {},
            )
        }
    }

    @Composable
    private fun Filters(state: FiltersUiState) {
        FiltersScreen(
            state = state,
            actions = FiltersActions(
                selectTab = {},
                update = {},
                setEnabled = { _, _ -> },
                subscribeEntry = {},
                subscribeUrl = {},
                remove = {},
                blockDomain = {},
                trust = {},
                untrust = {},
                checkUrl = { _, _, _ -> },
                clearUrlCheck = {},
                import = {},
                export = {},
                openEditor = {},
                openSettings = {},
                retry = {},
            ),
        )
    }
}
