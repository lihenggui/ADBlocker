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

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.RuleColors
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.highlightRules
import li.mercury.adblocker.core.ui.pageMargin

@Composable
internal fun RulesTab(state: FiltersUiState, actions: FiltersActions, onBlockDomain: () -> Unit, onTestUrl: () -> Unit) {
    val margin = pageMargin(LocalCompactWindow.current)
    val lines = state.userRules.lines().filter { it.isNotBlank() }
    val count = lines.count { !it.startsWith('!') }
    val skipped = state.rulesCheck?.issues?.size ?: 0
    val motion = MaterialTheme.motionScheme
    LazyColumn(contentPadding = PaddingValues(start = margin, end = margin, top = 12.dp, bottom = 24.dp), modifier = Modifier.fillMaxSize()) {
        item(key = "rules") {
            RulesPreviewCard(lines, count, skipped, state.rulesCheckUnavailable, actions.openEditor, Modifier.animatedRow(this, motion))
        }
        item(key = "tools-header") { SectionHeader(stringResource(R.string.feature_filters_tools), Modifier.animatedRow(this, motion)) }
        item(key = "tools") {
            val tools = listOf(
                Triple(AdbIcons.Block, R.string.feature_filters_block_domain, R.string.feature_filters_block_domain_supporting) to onBlockDomain,
                Triple(AdbIcons.Science, R.string.feature_filters_test_url, R.string.feature_filters_test_url_supporting) to onTestUrl,
                Triple(AdbIcons.Upload, R.string.feature_filters_import, R.string.feature_filters_import_supporting) to actions.import,
                Triple(AdbIcons.Download, R.string.feature_filters_export, R.string.feature_filters_export_supporting) to actions.export,
            )
            SegmentedGroup(tools.size, Modifier.animatedRow(this, motion)) { index ->
                val (spec, action) = tools[index]
                SegmentedItem(
                    index = index,
                    count = tools.size,
                    headline = stringResource(spec.second),
                    supporting = { Text(stringResource(spec.third)) },
                    leading = { Icon(spec.first, contentDescription = null, Modifier.size(20.dp)) },
                    onClick = action,
                )
            }
        }
        item(key = "syntax") {
            Text(
                stringResource(R.string.feature_filters_syntax_note),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.animatedRow(this, motion).padding(start = 16.dp, end = 16.dp, top = 12.dp),
            )
        }
    }
}

@Composable
private fun RulesPreviewCard(
    lines: List<String>,
    count: Int,
    skipped: Int,
    checkUnavailable: Boolean,
    onOpenEditor: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest), modifier = modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.feature_filters_my_rules), style = MaterialTheme.typography.titleMedium)
                    Text(
                        when {
                            checkUnavailable -> stringResource(
                                R.string.feature_filters_rules_count_with_status,
                                pluralStringResource(R.plurals.feature_filters_rules_count, count, count),
                                stringResource(R.string.feature_filters_rule_check_unavailable),
                            )

                            skipped > 0 -> pluralStringResource(R.plurals.feature_filters_rules_count_skipped, count, count, skipped)

                            else -> pluralStringResource(R.plurals.feature_filters_rules_count, count, count)
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Button(onClick = onOpenEditor) {
                    Icon(AdbIcons.Edit, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text(stringResource(R.string.feature_filters_edit))
                }
            }
            // A block of code, on the page's surface, once there are rules.
            Reveal(lines.isNotEmpty()) {
                Text(
                    highlightRules(lines.take(PREVIEW_LINES).joinToString("\n"), ruleColors()),
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace, lineHeight = 22.sp),
                    maxLines = PREVIEW_LINES,
                    softWrap = false,
                    modifier = Modifier
                        .padding(top = 12.dp)
                        .fillMaxWidth()
                        .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(8.dp))
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                )
            }
        }
    }
}

private const val PREVIEW_LINES = 6

@Composable
internal fun ruleColors() = RuleColors(
    comment = MaterialTheme.colorScheme.onSurfaceVariant,
    anchor = MaterialTheme.colorScheme.primary,
    option = MaterialTheme.colorScheme.tertiary,
    exception = MaterialTheme.colorScheme.secondary,
    cosmetic = MaterialTheme.colorScheme.onSurfaceVariant,
)

internal fun issueText(kind: RuleIssueKind) = when (kind) {
    RuleIssueKind.Cosmetic -> R.string.feature_filters_issue_cosmetic
    RuleIssueKind.Invalid -> R.string.feature_filters_issue_invalid
    RuleIssueKind.MalformedDirective -> R.string.feature_filters_issue_malformed_directive
    RuleIssueKind.UnsupportedInclude -> R.string.feature_filters_issue_unsupported_include
}
