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

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.data.CatalogEntry
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.ListCategory
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.ui.AdaptiveSheet
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedToggleItem
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.categoryIcon
import li.mercury.adblocker.core.ui.categoryName
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.core.ui.isolateLtr
import li.mercury.adblocker.core.ui.pageMargin
import li.mercury.adblocker.core.ui.relativeTime
import li.mercury.adblocker.core.ui.segmentedColors
import li.mercury.adblocker.core.ui.segmentedItems

internal fun listTitle(list: FilterList): String = FilterCatalog.byUrl(list.url)?.title ?: list.title ?: list.url.substringAfter("://").substringBefore('/')

@Composable
internal fun ListsTab(
    state: FiltersUiState,
    actions: FiltersActions,
    listState: LazyListState,
    countsBefore: Map<String, Int?>,
    onOpenList: (FilterList) -> Unit,
) {
    val margin = pageMargin(LocalCompactWindow.current)
    val motion = MaterialTheme.motionScheme
    val entries = state.catalog
    val groups = remember(entries) { entries.groupBy { it.category } }
    LazyColumn(state = listState, contentPadding = PaddingValues(start = margin, end = margin, top = 12.dp, bottom = 96.dp), modifier = Modifier.fillMaxSize()) {
        item(key = "summary") { Box(Modifier.animatedRow(this, motion)) { Summary(state, actions.update) } }
        item(key = "subscribed") {
            SectionHeader(stringResource(R.string.feature_filters_subscribed_header), Modifier.animatedRow(this, motion), trailing = state.lists.size.toString())
        }
        itemsIndexed(state.lists, key = { _, it -> it.url }) { index, list ->
            ListRow(
                list,
                index,
                state.lists.size,
                state,
                countsBefore[list.url],
                onToggle = { actions.setEnabled(list, it) },
                onClick = { onOpenList(list) },
                modifier = Modifier.animatedRow(this, motion),
            )
        }
        catalog(groups, motion, actions.subscribeEntry)
    }
}

/**
 * The rule count, and updating: one wavy progress for the whole update (M3:
 * one indicator for many items). In a filled card, the count in display
 * small, emphasized.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Summary(state: FiltersUiState, onUpdate: () -> Unit) {
    val updating = state.updating != null
    val countStyle = MaterialTheme.typography.displaySmallEmphasized
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Column {
                Text(
                    formatCount(state.totalRules),
                    style = countStyle.copy(fontFeatureSettings = "tnum"),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    autoSize = TextAutoSize.StepBased(maxFontSize = countStyle.fontSize),
                )
                Text(
                    state.lists.count { it.enabled }.let { pluralStringResource(R.plurals.feature_filters_rules_in_lists, it, it) },
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            FilledTonalButton(onClick = onUpdate, enabled = !updating) {
                Icon(AdbIcons.Refresh, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
                Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                Text(stringResource(if (updating) R.string.feature_filters_updating else R.string.feature_filters_update_now))
            }
            // The progress takes the place of when the lists were updated, and gives it back.
            FadeThrough(state, { it.updating != null }, Modifier.fillMaxWidth(), Alignment.CenterStart, "update") { state ->
                if (state.updating != null) {
                    val label = stringResource(R.string.feature_filters_update_progress)
                    LinearWavyProgressIndicator(progress = { state.progress }, modifier = Modifier.fillMaxWidth().semantics { contentDescription = label })
                } else {
                    LastUpdated(state)
                }
            }
        }
    }
}

@Composable
private fun LastUpdated(state: FiltersUiState) {
    val updated = state.lists.mapNotNull { it.updated }.maxOrNull()
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(AdbIcons.Schedule, contentDescription = null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            listOf(
                if (updated != null) {
                    stringResource(R.string.feature_filters_last_updated, relativeTime(updated))
                } else {
                    stringResource(R.string.feature_filters_never_updated)
                },
                scheduleText(state.schedule, state.wifiOnly),
            ).joinToString(" · "),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/**
 * One subscribed list: what it is, how many rules, and how the update goes.
 * While a list downloads its line is in primary; once updated, it says how
 * many rules it gained ([before]: its count as the update began).
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ListRow(
    list: FilterList,
    index: Int,
    count: Int,
    state: FiltersUiState,
    before: Int?,
    onToggle: (Boolean) -> Unit,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val entry = FilterCatalog.byUrl(list.url)
    val title = listTitle(list)
    val ruleCount = list.rules
    val rules = formatCount(ruleCount ?: 0)
    val added = if (before != null && ruleCount != null && ruleCount > before) ruleCount - before else null
    val updated = list.updated
    val updateState = state.updating?.get(list.url).takeIf { list.enabled }
    val status = when {
        !list.enabled -> stringResource(R.string.feature_filters_list_off)
        updateState == ListUpdateState.Fetching -> stringResource(R.string.feature_filters_list_fetching)
        updateState == ListUpdateState.Failed -> stringResource(R.string.feature_filters_list_failed)
        updateState != null && ruleCount != null -> stringResource(R.string.feature_filters_list_done, rules)
        list.error != null -> stringResource(R.string.feature_filters_list_error)
        ruleCount == null -> stringResource(R.string.feature_filters_list_pending)
        added != null -> stringResource(R.string.feature_filters_list_added, rules, formatCount(added))
        updated == null -> stringResource(R.string.feature_filters_list_rules, rules)
        else -> stringResource(R.string.feature_filters_list_detail, rules, relativeTime(updated))
    }
    val statusColor by animateColorAsState(
        when {
            updateState == ListUpdateState.Fetching -> MaterialTheme.colorScheme.primary
            updateState == ListUpdateState.Failed || (updateState == null && list.enabled && list.error != null) -> MaterialTheme.colorScheme.error
            else -> MaterialTheme.colorScheme.onSurfaceVariant
        },
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "list status",
    )
    Box(modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = onClick,
            shapes = ListItemDefaults.segmentedShapes(index, count),
            colors = segmentedColors(),
            leadingContent = { Icon(categoryIcon(entry?.category), contentDescription = null, Modifier.size(20.dp)) },
            supportingContent = { Text(status, color = statusColor, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            trailingContent = {
                Switch(
                    checked = list.enabled,
                    onCheckedChange = onToggle,
                    thumbContent = if (list.enabled) {
                        { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
                    } else {
                        null
                    },
                    modifier = Modifier.semantics { contentDescription = title },
                )
            },
            content = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        )
    }
}

/** The lists that can be added, by category, a row an item: a list subscribed to leaves its group as the others close up. */
private fun LazyListScope.catalog(groups: Map<ListCategory, List<CatalogEntry>>, motion: MotionScheme, onSubscribe: (CatalogEntry) -> Unit) {
    for ((category, group) in groups) {
        item(key = "cat-$category") {
            SectionHeader(stringResource(R.string.feature_filters_available, stringResource(categoryName(category))), Modifier.animatedRow(this, motion))
        }
        segmentedItems(group, key = { "cat-${it.url}" }) { index, count, entry -> CatalogRow(entry, index, count, onSubscribe) }
        if (group.any { it.cosmetic }) {
            item(key = "cat-note-$category") {
                Text(
                    stringResource(R.string.feature_filters_cosmetic_note),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.animatedRow(this, motion).padding(start = 16.dp, end = 16.dp, top = 8.dp),
                )
            }
        }
    }
}

/** Lists that can be added, each with a switch, as the subscribed ones have: turning it on subscribes and downloads it. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun CatalogGroup(group: List<CatalogEntry>, onSubscribe: (CatalogEntry) -> Unit, modifier: Modifier = Modifier) {
    SegmentedGroup(group.size, modifier) { index -> CatalogRow(group[index], index, group.size, onSubscribe) }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CatalogRow(entry: CatalogEntry, index: Int, count: Int, onSubscribe: (CatalogEntry) -> Unit) {
    val locale = LocalConfiguration.current.locales[0]
    val label = stringResource(R.string.feature_filters_subscribe, entry.title)
    SegmentedToggleItem(
        index = index,
        count = count,
        headline = entry.title,
        checked = false,
        onCheckedChange = { if (it) onSubscribe(entry) },
        supporting = {
            val description = entry.descriptions[locale.toLanguageTag()] ?: entry.descriptions[locale.language] ?: entry.descriptions["en"]
            Text(description ?: stringResource(categoryName(entry.category)), maxLines = 2, overflow = TextOverflow.Ellipsis)
        },
        leading = { Icon(categoryIcon(entry.category), contentDescription = null, Modifier.size(20.dp)) },
        trailing = { Switch(checked = false, onCheckedChange = null, modifier = Modifier.semantics { contentDescription = label }) },
    )
}

@Composable
internal fun ListInfoSheet(list: FilterList, onRemove: () -> Unit, onDismiss: () -> Unit) {
    AdaptiveSheet(onDismiss = onDismiss, title = listTitle(list)) {
        Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SelectionContainer {
                Text(list.url, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            list.rules?.let { Text(stringResource(R.string.feature_filters_list_rules, formatCount(it)), style = MaterialTheme.typography.bodyLarge) }
            list.updated?.let { Text(stringResource(R.string.feature_filters_last_updated, relativeTime(it)), style = MaterialTheme.typography.bodyLarge) }
            list.error?.let {
                Text(stringResource(R.string.feature_filters_list_last_error, isolateLtr(it)), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = onRemove) { Text(stringResource(R.string.feature_filters_remove_list)) }
            }
        }
    }
}

/** How the lists are kept up to date, as the settings say: "every day, on Wi-Fi only". */
@Composable
private fun scheduleText(schedule: UpdateInterval, wifiOnly: Boolean): String = when (schedule) {
    UpdateInterval.Never -> stringResource(R.string.feature_filters_schedule_never)

    else -> stringResource(
        if (wifiOnly) R.string.feature_filters_schedule_wifi else R.string.feature_filters_schedule,
        stringResource(
            when (schedule) {
                UpdateInterval.Daily -> R.string.feature_filters_every_day
                UpdateInterval.EveryThreeDays -> R.string.feature_filters_every_three_days
                else -> R.string.feature_filters_every_week
            },
        ),
    )
}
