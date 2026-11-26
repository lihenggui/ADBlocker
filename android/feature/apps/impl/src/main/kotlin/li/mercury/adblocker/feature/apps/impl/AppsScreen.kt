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

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.material3.AppBarWithSearch
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberSearchBarState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppListWithheld
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.ContentKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FailureView
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.appLabelArgument
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.core.ui.message
import li.mercury.adblocker.core.ui.pageMargin
import li.mercury.adblocker.core.ui.profileAppLabel
import li.mercury.adblocker.core.ui.rememberAppListAccess
import li.mercury.adblocker.core.ui.rememberNow
import li.mercury.adblocker.core.ui.segmentedColors
import li.mercury.adblocker.core.ui.timeLeft
import li.mercury.adblocker.core.ui.R as UiR

@Composable
fun AppsScreen(
    selectedPackage: String?,
    onOpenApp: (String) -> Unit,
    onOpenSettings: () -> Unit,
    viewModel: AppsViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    val allowAppList = rememberAppListAccess(state.listWithheld, viewModel::refresh)
    AppsScreen(
        state = state,
        onAllowAppList = allowAppList,
        selectedPackage = selectedPackage,
        onFilter = viewModel::setFilter,
        onToggle = { entry, on -> viewModel.setFiltered(entry, on) },
        onRetry = viewModel::refresh,
        onOpenApp = onOpenApp,
        onOpenSettings = onOpenSettings,
        modifier = modifier,
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun AppsScreen(
    state: AppsUiState,
    selectedPackage: String?,
    onFilter: (AppFilter) -> Unit,
    onToggle: (AppEntry, Boolean) -> Unit,
    onRetry: () -> Unit,
    onOpenApp: (String) -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    onAllowAppList: () -> Unit = {},
) {
    val searchState = rememberSearchBarState()
    val query = rememberTextFieldState()
    val margin = pageMargin(LocalCompactWindow.current)
    Scaffold(
        modifier = modifier,
        topBar = {
            // Search is what this page is for: finding one app among many.
            AppBarWithSearch(
                state = searchState,
                inputField = { SearchField(query, searchState) },
                actions = {
                    IconButton(onClick = onOpenSettings) {
                        Icon(AdbIcons.Settings, contentDescription = stringResource(R.string.feature_apps_settings))
                    }
                },
            )
            SearchResults(searchState, query, state, onToggle, onOpenApp)
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        FadeThrough(state, { ContentKind.of(it.loading, it.failure, empty = it.listWithheld) }, Modifier.padding(padding).fillMaxSize(), Alignment.Center, "apps") { state ->
            when {
                state.loading -> LoadingIndicator()
                state.failure != null -> FailureView(state.failure, onRetry)
                state.listWithheld -> AppListWithheld(onAllowAppList, onRetry)
                else -> AppsList(state, margin, selectedPackage, onFilter, onRetry, onToggle, onOpenApp)
            }
        }
    }
}

@Composable
private fun AppsList(
    state: AppsUiState,
    margin: Dp,
    selectedPackage: String?,
    onFilter: (AppFilter) -> Unit,
    onRetry: () -> Unit,
    onToggle: (AppEntry, Boolean) -> Unit,
    onOpenApp: (String) -> Unit,
) {
    val motion = MaterialTheme.motionScheme
    val shown = if (state.filter == AppFilter.All) state.entries else remember(state.entries, state.touched) { state.shown }
    val groups = if (state.filter == AppFilter.All) remember(shown) { shown.partition { it.isFiltered } } else null
    LazyColumn(
        contentPadding = PaddingValues(start = margin, end = margin, bottom = 24.dp),
        modifier = Modifier.fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth),
    ) {
        item(key = "chips") { Chips(state, onFilter, Modifier.animatedRow(this, motion)) }
        if (state.personalEligibilityUnavailable) {
            item(key = "personal-eligibility-unavailable") {
                InventoryNotice(UiR.string.core_ui_personal_eligibility_unavailable, onRetry, Modifier.animatedRow(this, motion))
            }
        }
        if (state.workInventoryUnavailable) {
            item(key = "work-inventory-unavailable") {
                InventoryNotice(R.string.feature_apps_work_inventory_unavailable, onRetry, Modifier.animatedRow(this, motion))
            }
        }
        appList(state, shown, groups, selectedPackage, onToggle, onOpenApp, onBrowseApps = { onFilter(AppFilter.All) })
    }
}

@Composable
private fun InventoryNotice(text: Int, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier.fillMaxWidth().padding(start = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            stringResource(text),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.weight(1f),
        )
        TextButton(onClick = onRetry) { Text(stringResource(R.string.feature_apps_work_inventory_retry)) }
    }
}

/** A single-select set of filter chips, introduced for screen readers as filters. */
@Composable
private fun Chips(state: AppsUiState, onFilter: (AppFilter) -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier.horizontalScroll(rememberScrollState()).padding(vertical = 4.dp).selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val chips = listOf(
            AppFilter.Filtered to stringResource(R.string.feature_apps_filtered, state.filteredCount),
            AppFilter.All to stringResource(R.string.feature_apps_all, state.entries.size),
        )
        for ((filter, label) in chips) {
            val selected = state.filter == filter
            FilterChip(
                selected = selected,
                onClick = { onFilter(filter) },
                label = { Text(label) },
                leadingIcon = if (selected) {
                    { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(FilterChipDefaults.IconSize)) }
                } else {
                    null
                },
                // Outline, not outline variant: 3:1 against the page (chip accessibility).
                border = FilterChipDefaults.filterChipBorder(
                    enabled = true,
                    selected = selected,
                    borderColor = MaterialTheme.colorScheme.outline,
                ),
                modifier = Modifier.semantics { role = Role.RadioButton },
            )
        }
    }
}

private fun LazyListScope.appList(
    state: AppsUiState,
    shown: List<AppEntry>,
    groups: Pair<List<AppEntry>, List<AppEntry>>?,
    selectedPackage: String?,
    onToggle: (AppEntry, Boolean) -> Unit,
    onOpenApp: (String) -> Unit,
    onBrowseApps: () -> Unit,
) {
    if (groups != null) {
        val (on, off) = groups
        group("filtered", R.string.feature_apps_section_filtered, on, state, selectedPackage, onToggle, onOpenApp)
        group("others", R.string.feature_apps_section_others, off, state, selectedPackage, onToggle, onOpenApp)
    } else if (shown.isEmpty()) {
        item(key = "empty") {
            Placeholder(
                icon = AdbIcons.Apps,
                title = stringResource(R.string.feature_apps_empty_filtered),
                text = stringResource(R.string.feature_apps_empty_filtered_text),
                modifier = Modifier.animatedRow(this, MaterialTheme.motionScheme).heightIn(min = 320.dp),
                action = {
                    Button(onClick = onBrowseApps) { Text(stringResource(R.string.feature_apps_browse_apps)) }
                },
            )
        }
    } else {
        item(key = "spacer") { Spacer(Modifier.animatedRow(this, MaterialTheme.motionScheme).height(8.dp)) }
        rows(shown, state, selectedPackage, onToggle, onOpenApp)
    }
}

private fun LazyListScope.group(
    key: String,
    title: Int,
    entries: List<AppEntry>,
    state: AppsUiState,
    selectedPackage: String?,
    onToggle: (AppEntry, Boolean) -> Unit,
    onOpenApp: (String) -> Unit,
) {
    if (entries.isEmpty()) return
    item(key = "header-$key") { SectionHeader(stringResource(title), Modifier.animatedRow(this, MaterialTheme.motionScheme), trailing = entries.size.toString()) }
    rows(entries, state, selectedPackage, onToggle, onOpenApp)
}

private fun LazyListScope.rows(
    entries: List<AppEntry>,
    state: AppsUiState,
    selectedPackage: String?,
    onToggle: (AppEntry, Boolean) -> Unit,
    onOpenApp: (String) -> Unit,
) {
    itemsIndexed(entries, key = { _, it -> it.appKey }) { index, entry ->
        AppRow(
            entry = entry,
            index = index,
            count = entries.size,
            counts = state.counts.map { it[entry.appKey] },
            selected = entry.appKey == selectedPackage,
            onToggle = onToggle,
            onClick = { onOpenApp(entry.appKey) },
            modifier = Modifier.animatedRow(this, MaterialTheme.motionScheme),
        )
    }
}

/**
 * One app: the row opens its page, and the switch starts or stops filtering it
 * at once. The row is selected only while the app's page is shown beside the
 * list (M3 list-detail).
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun AppRow(
    entry: AppEntry,
    index: Int,
    count: Int,
    counts: ReadState<AppCount?>,
    selected: Boolean,
    onToggle: (AppEntry, Boolean) -> Unit,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    member: SharedAppMember? = null,
) {
    val label = profileAppLabel(member?.label ?: entry.app.label, member?.packageName ?: entry.app.packageName, entry.app.profileSerial != null, entry.app.isSharedGroup)
    Box(modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            selected = selected && LocalShowsDetailBesideList.current,
            onClick = onClick,
            shapes = ListItemDefaults.segmentedShapes(index, count),
            colors = segmentedColors(),
            leadingContent = { AppIcon(entry.app, label) },
            supportingContent = {
                if (entry.app.sharedMembers.isEmpty()) {
                    AppStatus(entry, counts)
                } else {
                    Column {
                        AppStatus(entry, counts)
                        Text(
                            stringResource(UiR.string.core_ui_shared_app_members, entry.app.sharedMembers.joinToString { appLabelArgument(it.label, it.packageName) }),
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            },
            trailingContent = {
                if (entry.isSelected && entry.unavailableReason != null) {
                    TextButton(onClick = { onToggle(entry, false) }) {
                        Text(stringResource(UiR.string.core_ui_remove_app_selection))
                    }
                } else if (entry.inAllAppsScope && entry.canEnableFiltering) {
                    TextButton(onClick = { onToggle(entry, entry.excluded) }) {
                        Text(stringResource(if (entry.excluded) R.string.feature_apps_include else R.string.feature_apps_exclude))
                    }
                } else {
                    Switch(
                        checked = entry.isFiltered,
                        enabled = entry.isSelected || entry.canEnableFiltering,
                        onCheckedChange = { onToggle(entry, it) },
                        thumbContent = if (entry.isFiltered) {
                            { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
                        } else {
                            null
                        },
                        modifier = Modifier.semantics { contentDescription = label },
                    )
                }
            },
            content = { Text(label, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        )
    }
}

/** The app's filtering state, with an icon when paused. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AppStatus(entry: AppEntry, counts: ReadState<AppCount?>) {
    val filter = entry.filter
    val pausedUntil = filter?.pausedUntil
    val unavailable = entry.unavailableReason
    val work = entry.app.work
    val activePause = pausedUntil?.takeUnless { unavailable != null || work?.available == false }
    val text = when {
        unavailable != null -> stringResource(unavailable.message())
        work?.quiet == true -> stringResource(R.string.feature_apps_status_work_quiet)
        work?.available == false -> stringResource(R.string.feature_apps_status_work_unavailable)
        activePause != null -> stringResource(R.string.feature_apps_status_paused, timeLeft(activePause, rememberNow()))
        filter != null -> filteringStatus(filter, counts)
        entry.excluded -> stringResource(R.string.feature_apps_excluded)
        else -> stringResource(R.string.feature_apps_status_off)
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        if (activePause != null) Icon(AdbIcons.Timer, contentDescription = null, Modifier.size(16.dp))
        Text(text, maxLines = if (counts is ReadState.Unavailable) 2 else 1, overflow = TextOverflow.Ellipsis)
    }
}

/** The filtering mode is known even when its counters cannot be read. */
@Composable
private fun filteringStatus(filter: FilteredApp, counts: ReadState<AppCount?>): String {
    val mixed = filter.httpLevel == FilterLevel.Full && filter.level == FilterLevel.Hosts
    val hostsOnly = filter.level == FilterLevel.Hosts
    return when (counts) {
        is ReadState.Available -> {
            val text = when {
                mixed -> R.string.feature_apps_status_mixed
                hostsOnly -> R.string.feature_apps_status_hosts
                else -> R.string.feature_apps_status_filtered
            }
            stringResource(text, formatCount(counts.value?.today?.total ?: 0))
        }

        is ReadState.Unavailable, ReadState.Pending -> {
            val text = when {
                mixed -> R.string.feature_apps_status_mixed_no_count
                hostsOnly -> R.string.feature_apps_status_hosts_no_count
                else -> R.string.feature_apps_status_filtered_no_count
            }
            stringResource(text)
        }
    }
}
