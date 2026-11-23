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

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.material3.ExpandedDockedSearchBarWithGap
import androidx.compose.material3.ExpandedFullScreenContainedSearchBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SearchBarDefaults
import androidx.compose.material3.SearchBarState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.animatedRow

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SearchField(query: TextFieldState, searchState: SearchBarState) {
    val scope = rememberCoroutineScope()
    SearchBarDefaults.InputField(
        textFieldState = query,
        searchBarState = searchState,
        onSearch = { scope.launch { searchState.animateToCollapsed() } },
        placeholder = { Text(stringResource(R.string.feature_apps_search)) },
        leadingIcon = { Icon(AdbIcons.Search, contentDescription = null) },
    )
}

/**
 * The search view: full screen in compact windows, docked under the bar in
 * wider ones (M3 search); contained style, as recommended.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun SearchResults(
    searchState: SearchBarState,
    query: TextFieldState,
    state: AppsUiState,
    onToggle: (AppEntry, Boolean) -> Unit,
    onOpenApp: (String) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val text by snapshotFlow { query.text.toString() }.collectAsStateWithLifecycle("")
    val matches = remember(text, state.entries) {
        matchingAppRows(state.entries, text.trim())
    }
    val motion = MaterialTheme.motionScheme
    val results: @Composable () -> Unit = {
        LazyColumn(contentPadding = PaddingValues(16.dp)) {
            itemsIndexed(matches, key = { _, match -> match.key }) { index, match ->
                val entry = match.entry
                AppRow(
                    entry = entry,
                    index = index,
                    count = matches.size,
                    counts = state.counts.map { it[entry.appKey] },
                    selected = false,
                    onToggle = onToggle,
                    member = match.member,
                    onClick = {
                        scope.launch { searchState.animateToCollapsed() }
                        onOpenApp(entry.appKey)
                    },
                    modifier = Modifier.animatedRow(this, motion),
                )
            }
        }
    }
    if (LocalCompactWindow.current) {
        ExpandedFullScreenContainedSearchBar(state = searchState, inputField = { SearchField(query, searchState) }) { results() }
    } else {
        ExpandedDockedSearchBarWithGap(state = searchState, inputField = { SearchField(query, searchState) }) { results() }
    }
}

internal data class AppMatch(val entry: AppEntry, val member: SharedAppMember? = null) {
    val key: String get() = member?.let { "${entry.appKey}/${it.packageName}" } ?: entry.appKey
}

internal fun matchingAppRows(entries: List<AppEntry>, query: String): List<AppMatch> {
    if (query.isEmpty()) return emptyList()
    return entries.flatMap { entry ->
        val members = entry.app.sharedMembers.filter { it.label.contains(query, ignoreCase = true) || it.packageName.contains(query, ignoreCase = true) }
        if (members.isNotEmpty()) {
            members.map { AppMatch(entry, it) }
        } else if (entry.app.label.contains(query, ignoreCase = true) || entry.packageName.contains(query, ignoreCase = true)) {
            listOf(AppMatch(entry))
        } else {
            emptyList()
        }
    }
}
