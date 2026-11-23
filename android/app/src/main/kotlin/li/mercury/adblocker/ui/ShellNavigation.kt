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

package li.mercury.adblocker.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ShortNavigationBarDefaults
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDragHandle
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.layout.PaneExpansionAnchor
import androidx.compose.material3.adaptive.layout.PaneExpansionStateKey
import androidx.compose.material3.adaptive.layout.PaneScaffoldDirective
import androidx.compose.material3.adaptive.layout.rememberPaneExpansionState
import androidx.compose.material3.adaptive.navigation3.ListDetailSceneStrategy
import androidx.compose.material3.adaptive.navigation3.rememberListDetailSceneStrategy
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuite
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteDefaults
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteItem
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteScaffold
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteType
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.layout.layout
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp
import androidx.navigation3.runtime.NavEntry
import androidx.navigation3.runtime.NavKey
import androidx.window.core.layout.WindowSizeClass
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.launch
import li.mercury.adblocker.R
import li.mercury.adblocker.core.navigation.NavigationState
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.LocalHostsSnackbar
import li.mercury.adblocker.core.ui.LocalMessageScope
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.LocalSnackbarHostState
import li.mercury.adblocker.core.ui.LocalWideWindow
import li.mercury.adblocker.core.ui.PaneSurface
import li.mercury.adblocker.feature.activity.api.navigation.RequestNavKey
import li.mercury.adblocker.feature.appdetail.api.navigation.AppDetailNavKey
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3AdaptiveApi::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun rememberPaneStrategy(
    directive: PaneScaffoldDirective,
    listPaneWide: Boolean,
    onListPaneWide: (Boolean) -> Unit,
): ListDetailSceneStrategy<NavKey> {
    val listPaneWidth = directive.defaultPanePreferredWidth
    // The list can be widened, up to half the window, and back: the pane
    // drag handle between list and detail snaps to these (M3 panes).
    val anchors = remember(listPaneWidth) { listOf(PaneExpansionAnchor.Offset.fromStart(listPaneWidth), PaneExpansionAnchor.Proportion(0.5f)) }
    val (narrowAnchor, wideAnchor) = anchors
    val paneExpansionState = rememberPaneExpansionState(
        key = PaneExpansionStateKey.Default,
        anchors = anchors,
        // Where the user left it, the next time the app opens.
        initialAnchoredIndex = if (listPaneWide) 1 else 0,
    )
    LaunchedEffect(paneExpansionState, anchors) {
        snapshotFlow { paneExpansionState.currentAnchor }
            .filterNotNull()
            .collect { onListPaneWide(it == wideAnchor) }
    }
    return rememberListDetailSceneStrategy<NavKey>(
        directive = directive,
        paneExpansionState = paneExpansionState,
        paneExpansionDragHandle = { state ->
            val interactionSource = remember { MutableInteractionSource() }
            val coroutineScope = rememberCoroutineScope()
            val currentAnchor = state.currentAnchor
            val nextAnchor = when (currentAnchor) {
                narrowAnchor -> wideAnchor
                wideAnchor -> narrowAnchor
                else -> null
            }
            val handleDescription = stringResource(R.string.pane_drag_handle)
            val currentDescription = currentAnchor?.let { stringResource(R.string.pane_current_split, it.description) }
            val nextDescription = nextAnchor?.let { stringResource(R.string.pane_change_split, it.description) }
            VerticalDragHandle(
                modifier = Modifier.paneExpansionDraggable(
                    state,
                    LocalMinimumInteractiveComponentSize.current,
                    interactionSource,
                    // The draggable installs a clickable only when given an explicit onClick.
                    semanticsProperties = {
                        contentDescription = handleDescription
                        currentDescription?.let { stateDescription = it }
                        if (nextAnchor != null) {
                            onClick(label = nextDescription) {
                                coroutineScope.launch { state.animateTo(nextAnchor) }
                                true
                            }
                        }
                    },
                ),
                interactionSource = interactionSource,
            )
        },
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ShellNavigationItems(selectedRoot: NavKey, progress: Float?, navigator: Navigator, suiteType: NavigationSuiteType) {
    for (destination in TOP_LEVEL_DESTINATIONS) {
        val selected = selectedRoot == destination.key
        NavigationSuiteItem(
            selected = selected,
            onClick = { if (progress == null) navigator.navigate(destination.key) },
            icon = { Icon(if (selected) destination.selectedIcon else destination.icon, contentDescription = null) },
            label = { Text(stringResource(destination.label)) },
            navigationSuiteType = suiteType,
        )
    }
}

@Composable
internal fun ShellContent(
    besideList: Boolean,
    sizeClass: WindowSizeClass,
    singlePane: Boolean,
    snackbarHostState: SnackbarHostState,
    messageScope: CoroutineScope,
    content: @Composable () -> Unit,
) {
    CompositionLocalProvider(
        LocalShowsDetailBesideList provides besideList,
        LocalCompactWindow provides !sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_MEDIUM_LOWER_BOUND),
        LocalWideWindow provides !singlePane,
        LocalSnackbarHostState provides snackbarHostState,
        LocalMessageScope provides messageScope,
        content = content,
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CompactNavigation(
    navigationState: NavigationState,
    previousKey: NavKey?,
    progress: Float?,
    suiteType: NavigationSuiteType,
    navigationItems: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    val navigationColors = NavigationSuiteDefaults.colors()
    // Keep the system gesture bar outside the clipped reveal.
    val bottomInsets = ShortNavigationBarDefaults.windowInsets.only(WindowInsetsSides.Bottom)
    // A request overlays its list. Screens below it make room for their own controls.
    val resting = if (navigationState.currentKey in navigationState.topLevelKeys || navigationState.currentKey is RequestNavKey) 1f else 0f
    val previous = previousKey?.let { if (it in navigationState.topLevelKeys || it is RequestNavKey) 1f else 0f }
    val reveal = remember { Animatable(resting) }
    // The bar's height is a size: the standard scheme's spatial spec, which barely overshoots.
    val settle = MotionScheme.standard().fastSpatialSpec<Float>()
    // Composed again only when the bar comes or goes; its height is read while laying out.
    val revealing by remember { derivedStateOf { reveal.value > 0f } }
    LaunchedEffect(resting, previous, progress) {
        if (previous != null && progress != null) {
            reveal.snapTo(lerp(resting, previous, progress))
        } else {
            reveal.animateTo(resting, settle)
        }
    }
    Scaffold(
        modifier = modifier,
        containerColor = MaterialTheme.colorScheme.surface,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        bottomBar = {
            if (resting > 0f || previous == 1f || revealing) {
                NavigationSuite(
                    navigationSuiteType = suiteType,
                    colors = navigationColors,
                    modifier = Modifier
                        .background(navigationColors.shortNavigationBarContainerColor)
                        .windowInsetsPadding(bottomInsets)
                        .clipToBounds().layout { measurable, constraints ->
                            val bar = measurable.measure(constraints)
                            // A spring can pass its target by a fraction; the bar never grows past itself or below nothing.
                            layout(bar.width, (bar.height * reveal.value.coerceIn(0f, 1f)).roundToInt()) { bar.placeRelative(0, 0) }
                        },
                    content = navigationItems,
                )
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding)) { content() }
    }
}

/** Screens shown as the detail pane beside such a list. */
private fun NavKey.isDetail() = this is AppDetailNavKey || this is RequestNavKey

/** The widest a supporting pane may be (M3 standard side sheet). */
private val SIDE_PANE_WIDTH = 400.dp

/** The margin a [PaneSurface] keeps from the window's end. */
private val PANE_MARGIN = 24.dp

internal fun shellEntryProvider(
    provider: (NavKey) -> NavEntry<NavKey>,
    snackbarOwner: () -> NavKey?,
    besideList: () -> Boolean,
    extraLarge: () -> Boolean,
    sidePane: @Composable (String) -> Unit,
): (NavKey) -> NavEntry<NavKey> = { key ->
    val entry = provider(key)
    NavEntry(key, entry.contentKey, entry.metadata) {
        CompositionLocalProvider(LocalHostsSnackbar provides (key == snackbarOwner())) {
            when {
                !besideList() || !key.isDetail() -> entry.Content()

                extraLarge() && key is AppDetailNavKey -> Row {
                    PaneSurface(Modifier.weight(1f)) { entry.Content() }
                    PaneSurface(Modifier.width(SIDE_PANE_WIDTH + PANE_MARGIN)) { sidePane(key.packageName) }
                }

                else -> PaneSurface { entry.Content() }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ShellScaffold(
    navigationState: NavigationState,
    previousKey: NavKey?,
    progress: Float?,
    suiteType: NavigationSuiteType,
    navigationItems: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    val isBar = suiteType == NavigationSuiteType.ShortNavigationBarCompact ||
        suiteType == NavigationSuiteType.ShortNavigationBarMedium
    if (isBar) {
        CompactNavigation(navigationState, previousKey, progress, suiteType, navigationItems, modifier, content)
    } else {
        NavigationSuiteScaffold(
            modifier = modifier,
            navigationItems = navigationItems,
            navigationSuiteType = suiteType,
            containerColor = MaterialTheme.colorScheme.surface,
            content = content,
        )
    }
}
