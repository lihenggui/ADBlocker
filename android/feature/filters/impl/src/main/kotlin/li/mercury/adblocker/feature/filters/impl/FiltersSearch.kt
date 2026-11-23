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

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.material3.ExpandedDockedSearchBarWithGap
import androidx.compose.material3.ExpandedFullScreenContainedSearchBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SearchBarDefaults
import androidx.compose.material3.SearchBarState
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.categoryIcon
import li.mercury.adblocker.core.ui.highlightRules
import li.mercury.adblocker.core.ui.segmentedColors

/**
 * Searching the page: subscribed lists, lists to add, the user's rules and
 * trusted sites, as the query matches them. Full screen in compact windows,
 * docked under the bar in wider ones (M3 search).
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun FiltersSearch(
    searchState: SearchBarState,
    query: TextFieldState,
    state: FiltersUiState,
    actions: FiltersActions,
    onOpenList: (FilterList) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val close = { scope.launch { searchState.animateToCollapsed() } }
    val text by snapshotFlow { query.text.toString() }.collectAsStateWithLifecycle("")
    val inputField: @Composable () -> Unit = {
        SearchBarDefaults.InputField(
            textFieldState = query,
            searchBarState = searchState,
            onSearch = {},
            placeholder = { Text(stringResource(R.string.feature_filters_search_hint)) },
            leadingIcon = {
                IconButton(onClick = { close() }) { Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_filters_back)) }
            },
        )
    }
    val results: @Composable () -> Unit = {
        SearchResults(
            query = text.trim(),
            state = state,
            actions = actions,
            onOpenList = {
                close()
                onOpenList(it)
            },
            onOpenEditor = {
                close()
                actions.openEditor()
            },
        )
    }
    if (LocalCompactWindow.current) {
        ExpandedFullScreenContainedSearchBar(state = searchState, inputField = inputField) { results() }
    } else {
        ExpandedDockedSearchBarWithGap(state = searchState, inputField = inputField) { results() }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SearchResults(
    query: String,
    state: FiltersUiState,
    actions: FiltersActions,
    onOpenList: (FilterList) -> Unit,
    onOpenEditor: () -> Unit,
) {
    if (query.isEmpty()) return
    val subscribed = state.lists.filter { listTitle(it).contains(query, ignoreCase = true) || it.url.contains(query, ignoreCase = true) }
    val catalog = state.catalog.filter { it.title.contains(query, ignoreCase = true) || it.url.contains(query, ignoreCase = true) }
    val rules = state.userRules.lines().filter { it.isNotBlank() && it.contains(query, ignoreCase = true) }
    val trusted = state.trusted.filter { it.contains(query, ignoreCase = true) }
    val colors = ruleColors()
    val motion = MaterialTheme.motionScheme
    LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 16.dp), modifier = Modifier.fillMaxSize()) {
        if (subscribed.isEmpty() && catalog.isEmpty() && rules.isEmpty() && trusted.isEmpty()) {
            item(key = "none") {
                Text(
                    stringResource(R.string.feature_filters_search_none),
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.animatedRow(this, motion).padding(16.dp),
                )
            }
        }
        if (subscribed.isNotEmpty()) {
            item(key = "h-lists") { SectionHeader(stringResource(R.string.feature_filters_subscribed_header), Modifier.animatedRow(this, motion), trailing = subscribed.size.toString()) }
            item(key = "lists") {
                SegmentedGroup(subscribed.size, Modifier.animatedRow(this, motion)) { index ->
                    val list = subscribed[index]
                    SegmentedItem(
                        index = index,
                        count = subscribed.size,
                        headline = listTitle(list),
                        supporting = { Text(stringResource(if (list.enabled) R.string.feature_filters_list_on else R.string.feature_filters_list_off)) },
                        leading = { Icon(categoryIcon(FilterCatalog.byUrl(list.url)?.category), contentDescription = null, Modifier.size(20.dp)) },
                        onClick = { onOpenList(list) },
                    )
                }
            }
        }
        if (catalog.isNotEmpty()) {
            item(key = "h-catalog") { SectionHeader(stringResource(R.string.feature_filters_search_available), Modifier.animatedRow(this, motion), trailing = catalog.size.toString()) }
            item(key = "catalog") { CatalogGroup(catalog, actions.subscribeEntry, Modifier.animatedRow(this, motion)) }
        }
        if (rules.isNotEmpty()) {
            item(key = "h-rules") { SectionHeader(stringResource(R.string.feature_filters_my_rules), Modifier.animatedRow(this, motion), trailing = rules.size.toString()) }
            item(key = "rules") {
                SegmentedGroup(rules.size, Modifier.animatedRow(this, motion)) { index ->
                    SegmentedListItem(
                        onClick = onOpenEditor,
                        shapes = ListItemDefaults.segmentedShapes(index, rules.size),
                        colors = segmentedColors(),
                        content = {
                            Text(highlightRules(rules[index], colors), style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        },
                    )
                }
            }
        }
        if (trusted.isNotEmpty()) {
            item(key = "h-trusted") { SectionHeader(stringResource(R.string.feature_filters_tab_trusted), Modifier.animatedRow(this, motion), trailing = trusted.size.toString()) }
            item(key = "trusted") { TrustedGroup(trusted, actions.untrust, Modifier.animatedRow(this, motion)) }
        }
    }
}
