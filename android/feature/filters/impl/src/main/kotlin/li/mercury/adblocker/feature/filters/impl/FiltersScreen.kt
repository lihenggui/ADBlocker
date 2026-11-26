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

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallExtendedFloatingActionButton
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberSearchBarState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.LayoutDirection
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.CatalogEntry
import li.mercury.adblocker.core.data.repository.HostInput
import li.mercury.adblocker.core.data.repository.hostOf
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.designsystem.motion.lateralTransform
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.ContentKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FailureView
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.Pop

@Composable
fun FiltersScreen(
    onOpenEditor: () -> Unit,
    onOpenSettings: () -> Unit,
    viewModel: FiltersViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    val importer = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> uri?.let(viewModel::import) }
    val exporter = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri -> uri?.let(viewModel::export) }
    FiltersScreen(
        state = state,
        actions = FiltersActions(
            selectTab = viewModel::selectTab,
            update = viewModel::updateLists,
            setEnabled = viewModel::setEnabled,
            subscribeEntry = viewModel::subscribe,
            subscribeUrl = viewModel::subscribe,
            remove = viewModel::remove,
            blockDomain = viewModel::blockDomain,
            trust = viewModel::trust,
            untrust = viewModel::untrust,
            checkUrl = viewModel::checkUrl,
            clearUrlCheck = viewModel::clearUrlCheck,
            import = { importer.launch(arrayOf("text/plain")) },
            export = { exporter.launch("adblocker-rules.txt") },
            openEditor = onOpenEditor,
            openSettings = onOpenSettings,
            retry = viewModel::refresh,
        ),
        modifier = modifier,
    )
}

internal data class FiltersActions(
    val selectTab: (FiltersTab) -> Unit,
    val update: () -> Unit,
    val setEnabled: (FilterList, Boolean) -> Unit,
    val subscribeEntry: (CatalogEntry) -> Unit,
    val subscribeUrl: (String) -> Unit,
    val remove: (FilterList) -> Unit,
    val blockDomain: (String) -> Unit,
    val trust: (String) -> Unit,
    val untrust: (String) -> Unit,
    val checkUrl: (String, String, String) -> Unit,
    val clearUrlCheck: () -> Unit,
    val import: () -> Unit,
    val export: () -> Unit,
    val openEditor: () -> Unit,
    val openSettings: () -> Unit,
    val retry: () -> Unit,
)

/** Which sheet is open over the page. */
private sealed interface Overlay {
    data object AddList : Overlay

    data object BlockDomain : Overlay

    data object Trust : Overlay

    data object TestUrl : Overlay

    data class ListInfo(val list: FilterList) : Overlay
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun FiltersScreen(state: FiltersUiState, actions: FiltersActions, modifier: Modifier = Modifier) {
    var overlay by remember { mutableStateOf<Overlay?>(null) }
    val listsState = rememberLazyListState()
    val searchState = rememberSearchBarState()
    val query = rememberTextFieldState()
    val scope = rememberCoroutineScope()
    // The rule counts as an update starts, to tell afterwards how many each list gained.
    var countsBefore by remember { mutableStateOf<Map<String, Int?>>(emptyMap()) }
    val isRtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val updating = state.updating != null
    LaunchedEffect(updating) { if (updating) countsBefore = state.lists.associate { it.url to it.rules } }
    // The FAB folds to its icon while the list moves on, and opens again when it comes back (M3 FAB).
    val fabExpanded by remember { derivedStateOf { listsState.firstVisibleItemIndex == 0 || !listsState.lastScrolledForward } }
    Scaffold(
        modifier = modifier,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_filters_title)) },
                actions = {
                    IconButton(onClick = { scope.launch { searchState.animateToExpanded() } }) {
                        Icon(AdbIcons.Search, contentDescription = stringResource(R.string.feature_filters_search))
                    }
                    IconButton(onClick = actions.openSettings) { Icon(AdbIcons.Settings, contentDescription = stringResource(R.string.feature_filters_settings)) }
                },
            )
            FiltersSearch(searchState, query, state, actions, onOpenList = { overlay = Overlay.ListInfo(it) })
        },
        // One FAB, for the one constructive action of the lists tab. The
        // Scaffold puts snackbars above it.
        floatingActionButton = {
            // It goes with the lists tab, and arrives with the lists.
            Pop(state.tab == FiltersTab.Lists && state.failure == null && !state.loading) {
                SmallExtendedFloatingActionButton(
                    text = { Text(stringResource(R.string.feature_filters_add_list)) },
                    icon = { Icon(AdbIcons.Add, contentDescription = null) },
                    onClick = { overlay = Overlay.AddList },
                    expanded = fabExpanded,
                )
            }
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth)) {
            PrimaryTabRow(selectedTabIndex = state.tab.ordinal) {
                FiltersTab.entries.forEach { tab ->
                    Tab(
                        selected = state.tab == tab,
                        onClick = { actions.selectTab(tab) },
                        // Unselected tabs in on surface variant, the selected one in primary (M3 tabs).
                        unselectedContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        text = { Text(stringResource(tabLabel(tab))) },
                    )
                }
            }
            FadeThrough(state, { ContentKind.of(it.loading, it.failure) }, Modifier.fillMaxSize(), Alignment.Center, "filters") { state ->
                when {
                    state.loading -> LoadingIndicator()

                    state.failure != null -> FailureView(state.failure, actions.retry)

                    // Peers: the lateral pattern, sliding together, no fade.
                    else -> AnimatedContent(
                        targetState = state.tab,
                        transitionSpec = { lateralTransform(forward = targetState.ordinal > initialState.ordinal, isRtl) },
                        label = "tabs",
                    ) { tab ->
                        when (tab) {
                            FiltersTab.Lists -> ListsTab(state, actions, listsState, countsBefore) { overlay = Overlay.ListInfo(it) }
                            FiltersTab.Rules -> RulesTab(state, actions, onBlockDomain = { overlay = Overlay.BlockDomain }, onTestUrl = { overlay = Overlay.TestUrl })
                            FiltersTab.Trusted -> TrustedTab(state, actions) { overlay = Overlay.Trust }
                        }
                    }
                }
            }
        }
    }
    FiltersOverlay(overlay, state, actions) { overlay = null }
}

@Composable
private fun FiltersOverlay(overlay: Overlay?, state: FiltersUiState, actions: FiltersActions, onDismiss: () -> Unit) {
    when (overlay) {
        null -> Unit

        Overlay.AddList -> TextInputSheet(
            title = stringResource(R.string.feature_filters_add_list),
            text = stringResource(R.string.feature_filters_add_list_text),
            label = stringResource(R.string.feature_filters_add_list_label),
            confirm = stringResource(R.string.feature_filters_add),
            onConfirm = actions.subscribeUrl,
            onDismiss = onDismiss,
        )

        Overlay.BlockDomain -> TextInputSheet(
            title = stringResource(R.string.feature_filters_block_domain),
            text = stringResource(R.string.feature_filters_block_domain_text),
            label = stringResource(R.string.feature_filters_domain_label),
            confirm = stringResource(R.string.feature_filters_block),
            onConfirm = actions.blockDomain,
            onDismiss = onDismiss,
            preview = { value -> hostOf(value, HostInput.RulePreview)?.let { stringResource(R.string.feature_filters_block_preview, "||$it^") } },
        )

        Overlay.Trust -> TextInputSheet(
            title = stringResource(R.string.feature_filters_add_trusted),
            text = stringResource(R.string.feature_filters_trusted_text),
            label = stringResource(R.string.feature_filters_domain_label),
            confirm = stringResource(R.string.feature_filters_add),
            onConfirm = actions.trust,
            onDismiss = onDismiss,
        )

        Overlay.TestUrl -> TestUrlSheet(state.urlCheck, actions.checkUrl) {
            actions.clearUrlCheck()
            onDismiss()
        }

        is Overlay.ListInfo -> ListInfoSheet(overlay.list, onRemove = {
            actions.remove(overlay.list)
            onDismiss()
        }, onDismiss = onDismiss)
    }
}

private fun tabLabel(tab: FiltersTab) = when (tab) {
    FiltersTab.Lists -> R.string.feature_filters_tab_lists
    FiltersTab.Rules -> R.string.feature_filters_tab_rules
    FiltersTab.Trusted -> R.string.feature_filters_tab_trusted
}
