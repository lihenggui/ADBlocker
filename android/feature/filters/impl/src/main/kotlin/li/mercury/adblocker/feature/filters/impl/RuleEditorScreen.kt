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

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.OutputTransformation
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FloatingToolbarDefaults
import androidx.compose.material3.HorizontalFloatingToolbar
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.ui.AdaptiveSheet
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.ContentKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FailureView
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.highlightRules
import li.mercury.adblocker.core.ui.pageMargin

@Composable
fun RuleEditorScreen(
    onBack: () -> Unit,
    viewModel: RuleEditorViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    LifecycleStartEffect(viewModel) {
        viewModel.startEditing()
        // The process can end while the screen is away, before typing paused.
        onStopOrDispose { viewModel.flushDraft() }
    }
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    val done = state.done && !state.changed
    LaunchedEffect(done) {
        val latest = viewModel.uiState.value
        if (done && latest.done && !latest.changed) onBack()
    }
    RuleEditorScreen(
        state = state,
        urlCheck = state.urlCheck,
        onUndo = viewModel::undo,
        onRedo = viewModel::redo,
        onSave = viewModel::save,
        onCheckUrl = viewModel::checkUrl,
        onClearUrlCheck = viewModel::clearUrlCheck,
        onBack = onBack,
        onDiscard = viewModel::discard,
        onRetry = viewModel::load,
        modifier = modifier,
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun RuleEditorScreen(
    state: RuleEditorUiState,
    urlCheck: RuleCheckResult?,
    onUndo: () -> Unit,
    onRedo: () -> Unit,
    onSave: () -> Unit,
    onCheckUrl: (String, String, String) -> Unit,
    onClearUrlCheck: () -> Unit,
    onBack: () -> Unit,
    onRetry: () -> Unit,
    modifier: Modifier = Modifier,
    onDiscard: () -> Unit = onBack,
) {
    var testing by rememberSaveable { mutableStateOf(false) }
    var syntaxHelp by rememberSaveable { mutableStateOf(false) }
    var confirmLeave by rememberSaveable { mutableStateOf(false) }
    val leave = {
        if (!state.saving) {
            if (state.changed) confirmLeave = true else onBack()
        }
    }
    BackHandler(enabled = state.saving || state.changed, onBack = leave)
    Scaffold(
        modifier = modifier,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_filters_my_rules)) },
                navigationIcon = {
                    IconButton(onClick = leave, enabled = !state.saving) { Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_filters_back)) }
                },
                actions = {
                    IconButton(onClick = { syntaxHelp = true }) { Icon(AdbIcons.Info, contentDescription = stringResource(R.string.feature_filters_syntax)) }
                },
            )
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        FadeThrough(state, { ContentKind.of(it.loading, it.failure) }, Modifier.padding(padding).fillMaxSize().imePadding(), Alignment.Center, "editor") { state ->
            when {
                state.loading -> LoadingIndicator()

                state.failure != null -> FailureView(state.failure, onRetry)

                else -> Box(Modifier.fillMaxSize()) {
                    Editor(state)
                    EditorToolbar(
                        state = state,
                        onUndo = onUndo,
                        onRedo = onRedo,
                        onSave = onSave,
                        onTestUrl = { testing = true },
                        modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp),
                    )
                }
            }
        }
    }
    if (testing) {
        TestUrlSheet(urlCheck, onCheckUrl) {
            onClearUrlCheck()
            testing = false
        }
    }
    if (syntaxHelp) SyntaxSheet { syntaxHelp = false }
    if (confirmLeave) {
        AlertDialog(
            onDismissRequest = { confirmLeave = false },
            title = { Text(stringResource(R.string.feature_filters_discard_title)) },
            text = {
                val explanation = if (state.discardFailed) R.string.feature_filters_discard_failed else R.string.feature_filters_discard_text
                Text(stringResource(explanation))
            },
            confirmButton = {
                TextButton(onClick = onDiscard, enabled = !state.saving) { Text(stringResource(R.string.feature_filters_discard)) }
            },
            dismissButton = { TextButton(onClick = { confirmLeave = false }) { Text(stringResource(R.string.feature_filters_keep_editing)) } },
        )
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun EditorToolbar(
    state: RuleEditorUiState,
    onUndo: () -> Unit,
    onRedo: () -> Unit,
    onSave: () -> Unit,
    onTestUrl: () -> Unit,
    modifier: Modifier = Modifier,
) {
    // The page's actions, on a vibrant floating toolbar: it marks
    // that the page is for editing. The FAB is the one emphasized action.
    HorizontalFloatingToolbar(
        expanded = true,
        colors = FloatingToolbarDefaults.vibrantFloatingToolbarColors(),
        floatingActionButton = {
            FloatingToolbarDefaults.VibrantFloatingActionButton(onClick = onSave) {
                // Saving is a short wait: the Expressive loading indicator, in the button's color.
                FadeThrough(state.saving, { it }, contentAlignment = Alignment.Center, label = "save") { saving ->
                    if (saving) {
                        LoadingIndicator(Modifier.size(24.dp), color = LocalContentColor.current)
                    } else {
                        Icon(AdbIcons.Check, contentDescription = stringResource(R.string.feature_filters_save))
                    }
                }
            }
        },
        modifier = modifier,
    ) {
        IconButton(onClick = onUndo, enabled = state.canUndo) { Icon(AdbIcons.Undo, contentDescription = stringResource(R.string.feature_filters_undo)) }
        IconButton(onClick = onRedo, enabled = state.canRedo) { Icon(AdbIcons.Redo, contentDescription = stringResource(R.string.feature_filters_redo)) }
        IconButton(onClick = onTestUrl) { Icon(AdbIcons.Science, contentDescription = stringResource(R.string.feature_filters_test_url)) }
    }
}

/** Lines do not wrap, so that the line numbers stay beside their lines; the text scrolls sideways instead. */
@Composable
private fun Editor(state: RuleEditorUiState) {
    val margin = pageMargin(LocalCompactWindow.current)
    val colors = ruleColors()
    val textStyle = TextStyle(
        fontFamily = FontFamily.Monospace,
        fontSize = 14.sp,
        lineHeight = 22.sp,
        color = MaterialTheme.colorScheme.onSurface,
    )
    val lineCount = state.text.text.count { it == '\n' } + 1
    val lineHeight = 22.sp
    val issueTint = MaterialTheme.colorScheme.tertiary.copy(alpha = ISSUE_TINT_ALPHA)
    val diagnostics = state.currentDiagnostics
    val issues = diagnostics?.issues.orEmpty()
    Column(Modifier.fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth).verticalScroll(rememberScrollState()).padding(horizontal = margin)) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surfaceContainer, RoundedCornerShape(16.dp))
                .padding(vertical = 12.dp),
        ) {
            Column(Modifier.width(44.dp).padding(end = 8.dp)) {
                for (line in 1..lineCount) {
                    val issue = issues[line]
                    Row(Modifier.height(with(LocalDensity.current) { 22.sp.toDp() }).fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.End) {
                        if (issue != null) Icon(AdbIcons.Warning, contentDescription = null, Modifier.size(14.dp), tint = MaterialTheme.colorScheme.tertiary)
                        Text(line.toString(), style = textStyle.copy(color = MaterialTheme.colorScheme.outline, fontSize = 12.sp), textAlign = TextAlign.End)
                    }
                }
            }
            BasicTextField(
                state = state.text,
                textStyle = textStyle,
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                outputTransformation = remember(colors) {
                    OutputTransformation {
                        for ((style, start, end) in highlightRules(asCharSequence().toString(), colors).spanStyles) {
                            addStyle(style, start, end)
                        }
                    }
                },
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                modifier = Modifier
                    .weight(1f)
                    // A line that will not be applied is tinted across the editor, behind its text.
                    .drawBehind {
                        val line = lineHeight.toPx()
                        for (number in issues.keys) {
                            drawRoundRect(issueTint, topLeft = Offset(0f, (number - 1) * line), size = Size(size.width, line), cornerRadius = CornerRadius(4.dp.toPx()))
                        }
                    }
                    .horizontalScroll(rememberScrollState())
                    .padding(end = 12.dp),
            )
        }
        EditorNotices(diagnostics)
        // Room for the toolbar over the end of the text.
        Box(Modifier.height(112.dp))
    }
}

@Composable
private fun EditorNotices(diagnostics: RuleEditorDiagnostics?) {
    // The check runs after typing stops: what it finds grows in under the rules.
    RevealNotNull(diagnostics?.takeIf { it.issues?.isNotEmpty() != false }) { Diagnostics(it) }
}

@Composable
private fun Diagnostics(diagnostics: RuleEditorDiagnostics) {
    val issues = diagnostics.issues
    if (issues == null) {
        Text(
            stringResource(R.string.feature_filters_rule_check_unavailable),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(top = 12.dp),
        )
    } else {
        val issue = issues.values.minByOrNull { it.line } ?: return
        Card(
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.tertiaryContainer, contentColor = MaterialTheme.colorScheme.onTertiaryContainer),
            modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
        ) {
            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(AdbIcons.Warning, contentDescription = null, Modifier.size(20.dp))
                // Emphasize the line number so the user can find it in the editor.
                val lead = stringResource(R.string.feature_filters_issue_line, issue.line)
                val separator = stringResource(R.string.feature_filters_issue_separator)
                val additionalIssues = issues.size - 1
                val more = if (additionalIssues > 0) {
                    " " + pluralStringResource(R.plurals.feature_filters_more_issues, additionalIssues, additionalIssues)
                } else {
                    ""
                }
                val reason = ruleIssueText(issue)
                Text(
                    buildAnnotatedString {
                        withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(lead) }
                        append(separator)
                        append(reason)
                        append(more)
                    },
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
    }
}

/** How strongly a line that will not be applied is tinted. */
private const val ISSUE_TINT_ALPHA = 0.12f

/** The syntax the editor reads, in short: what each kind of rule looks like. */
@Composable
private fun SyntaxSheet(onDismiss: () -> Unit) {
    val colors = ruleColors()
    val examples = listOf(
        "||ads.example^" to R.string.feature_filters_syntax_host,
        "||track.example^\$third-party" to R.string.feature_filters_syntax_option,
        "||ads.example^\$app=com.example.app" to R.string.feature_filters_syntax_app,
        "/banner/*/ad_\$image" to R.string.feature_filters_syntax_path,
        "@@||cdn.example/player.js^" to R.string.feature_filters_syntax_exception,
        "example.com##.ad-banner" to R.string.feature_filters_syntax_cosmetic,
        "! A comment" to R.string.feature_filters_syntax_comment,
    )
    AdaptiveSheet(onDismiss = onDismiss, title = stringResource(R.string.feature_filters_syntax)) {
        Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(stringResource(R.string.feature_filters_syntax_text), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            for ((rule, meaning) in examples) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(
                        highlightRules(rule, colors),
                        style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerHighest, RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
                    )
                    Text(stringResource(meaning), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
    }
}
