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

import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.WindowAdaptiveInfo
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfoV2
import androidx.compose.material3.adaptive.layout.PaneScaffoldDirective
import androidx.compose.material3.adaptive.layout.calculatePaneScaffoldDirective
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteScaffoldDefaults
import androidx.compose.material3.adaptive.navigationsuite.NavigationSuiteType
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.scene.NavigationBackHandler
import androidx.navigation3.scene.SceneInfo
import androidx.navigation3.scene.SceneState
import androidx.navigation3.scene.rememberNavigationEventState
import androidx.navigation3.scene.rememberSceneState
import androidx.navigation3.ui.NavDisplay
import androidx.navigationevent.NavigationEventTransitionState.InProgress
import androidx.navigationevent.compose.NavigationEventState
import androidx.window.core.layout.WindowSizeClass
import li.mercury.adblocker.R
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.designsystem.motion.rememberScreenTransitions
import li.mercury.adblocker.core.navigation.BottomSheetSceneStrategy
import li.mercury.adblocker.core.navigation.NavigationState
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.core.navigation.rememberNavigationState
import li.mercury.adblocker.core.navigation.toEntries
import li.mercury.adblocker.feature.activity.api.navigation.ActivityNavKey
import li.mercury.adblocker.feature.activity.api.navigation.RequestNavKey
import li.mercury.adblocker.feature.activity.impl.AppRequestsPane
import li.mercury.adblocker.feature.activity.impl.AppRequestsViewModel
import li.mercury.adblocker.feature.activity.impl.navigation.activityEntries
import li.mercury.adblocker.feature.appdetail.impl.navigation.appDetailEntry
import li.mercury.adblocker.feature.apps.api.navigation.AppsNavKey
import li.mercury.adblocker.feature.apps.impl.navigation.appsEntry
import li.mercury.adblocker.feature.filters.api.navigation.FiltersNavKey
import li.mercury.adblocker.feature.filters.impl.navigation.filtersEntries
import li.mercury.adblocker.feature.https.impl.navigation.httpsEntry
import li.mercury.adblocker.feature.overview.api.navigation.OverviewNavKey
import li.mercury.adblocker.feature.overview.impl.navigation.overviewEntry
import li.mercury.adblocker.feature.settings.impl.navigation.settingsEntry

/** A destination in the navigation bar or rail. */
internal class TopLevelDestination(
    val key: NavKey,
    @StringRes val label: Int,
    val icon: ImageVector,
    val selectedIcon: ImageVector,
)

/**
 * Whether it works, what it filters, with what, and what it did: four
 * destinations, as Material allows three to five. The selected one has a
 * filled icon.
 */
internal val TOP_LEVEL_DESTINATIONS = listOf(
    TopLevelDestination(OverviewNavKey, R.string.nav_overview, AdbIcons.Shield, AdbIcons.ShieldFilled),
    TopLevelDestination(AppsNavKey, R.string.nav_apps, AdbIcons.Apps, AdbIcons.AppsFilled),
    TopLevelDestination(FiltersNavKey, R.string.nav_filters, AdbIcons.FilterAlt, AdbIcons.FilterAltFilled),
    TopLevelDestination(ActivityNavKey, R.string.nav_activity, AdbIcons.ReceiptLong, AdbIcons.ReceiptLongFilled),
)

/** Destinations shown as the list pane of a list-detail layout. */
private val LIST_PANE_KEYS = setOf<NavKey>(AppsNavKey, ActivityNavKey)

/**
 * Root of the UI: the navigation bar or rail, and the screens.
 *
 * Compact windows show one pane and a navigation bar; medium ones a
 * collapsed navigation rail; from the expanded breakpoint, a list beside
 * what it opens; from the large one, an expanded rail with labels; from the
 * extra large one, an app's recorded requests beside its page.
 */
@Composable
fun ADBlockerApp(
    modifier: Modifier = Modifier,
    /** The list pane was widened to half the window, as last left. */
    listPaneWide: Boolean = false,
    onListPaneWide: (Boolean) -> Unit = {},
) {
    val topLevelKeys = remember { TOP_LEVEL_DESTINATIONS.map { it.key }.toSet() }
    val navigationState = rememberNavigationState(startKey = OverviewNavKey, topLevelKeys = topLevelKeys)
    val navigator = remember(navigationState) { Navigator(navigationState) }
    ADBlockerShell(
        navigationState = navigationState,
        navigator = navigator,
        adaptiveInfo = currentWindowAdaptiveInfoV2(),
        modifier = modifier,
        listPaneWide = listPaneWide,
        onListPaneWide = onListPaneWide,
        sidePane = { packageName ->
            val viewModel = hiltViewModel<AppRequestsViewModel, AppRequestsViewModel.Factory>(key = "requests:$packageName") { it.create(packageName) }
            AppRequestsPane(viewModel, onOpenActivity = { navigator.navigate(ActivityNavKey) })
        },
    ) {
        overviewEntry(navigator)
        appsEntry(navigator)
        appDetailEntry(navigator)
        filtersEntries(navigator)
        activityEntries(navigator)
        settingsEntry(navigator)
        httpsEntry(navigator)
    }
}

/**
 * The shell around the screens, for any window: what [ADBlockerApp] shows,
 * with the screens [entries] gives, so that it can be shown with screens
 * that need no view models.
 */
@OptIn(ExperimentalMaterial3AdaptiveApi::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ADBlockerShell(
    navigationState: NavigationState,
    navigator: Navigator,
    adaptiveInfo: WindowAdaptiveInfo,
    modifier: Modifier = Modifier,
    listPaneWide: Boolean = false,
    onListPaneWide: (Boolean) -> Unit = {},
    /** What is beside an app's page in extra large windows: its recorded requests. */
    sidePane: @Composable (packageName: String) -> Unit = {},
    entries: EntryProviderScope<NavKey>.() -> Unit,
) {
    val sizeClass = adaptiveInfo.windowSizeClass
    val directive = remember(adaptiveInfo) { paneDirective(adaptiveInfo) }
    val listDetailStrategy = rememberPaneStrategy(directive, listPaneWide, onListPaneWide)
    val singlePane = directive.maxHorizontalPartitions == 1
    // Material keeps bottom sheets to windows with a single pane; wider ones
    // show a request as a pane instead.
    val bottomSheetStrategy = remember(singlePane) { BottomSheetSceneStrategy<NavKey>(enabled = singlePane) }

    // The screen that shows the snackbar: the front one, or under a sheet
    // the one it covers.
    val snackbarOwner by remember(navigationState, singlePane) {
        derivedStateOf {
            val stack = navigationState.currentSubStack
            val last = stack.last()
            if (singlePane && last is RequestNavKey) stack.getOrNull(stack.size - 2) else last
        }
    }
    // A list and its detail share the window only if a list is on the stack.
    val listOnStack = navigationState.currentSubStack.any { it in LIST_PANE_KEYS }
    val besideList by rememberUpdatedState(!singlePane && listOnStack)
    val extraLarge by rememberUpdatedState(sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_EXTRA_LARGE_LOWER_BOUND))
    val provider = remember(entries, sidePane, navigationState, singlePane) {
        shellEntryProvider(entryProvider(builder = entries), { snackbarOwner }, { besideList }, { extraLarge }, sidePane)
    }

    val sceneState = rememberSceneState(
        entries = navigationState.toEntries(provider),
        sceneStrategies = listOf(bottomSheetStrategy, listDetailStrategy),
        onBack = navigator::goBack,
    )
    val navigationEventState = rememberNavigationEventState(sceneState)
    NavigationBackHandler(sceneState, navigationEventState, onBackCompleted = navigator::goBack)

    val snackbarHostState = remember { SnackbarHostState() }
    val messageScope = rememberCoroutineScope()
    ShellWithBackProgress(
        navigationState,
        navigator,
        navigationEventState,
        sceneState.currentScene.previousEntries.size,
        navigationSuiteType(adaptiveInfo),
        modifier,
    ) {
        ShellContent(besideList, sizeClass, singlePane, snackbarHostState, messageScope) {
            ShellNavDisplay(
                sceneState = sceneState,
                navigationEventState = navigationEventState,
                movedBetweenTopLevels = { navigator.movedBetweenTopLevels },
                backLeavesTopLevel = { navigator.backLeavesTopLevel },
            )
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ShellWithBackProgress(
    navigationState: NavigationState,
    navigator: Navigator,
    navigationEventState: NavigationEventState<SceneInfo<NavKey>>,
    previousSize: Int,
    suiteType: NavigationSuiteType,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    // The gesture updates the scaffold without rebuilding the decorated entries.
    val progress = (navigationEventState.transitionState as? InProgress)?.latestEvent?.progress
    val previous by remember(navigationState, previousSize) {
        derivedStateOf {
            // SceneState describes the retained prefix of the decorated back stack.
            var remaining = previousSize
            navigationState.topLevelStack.firstNotNullOfOrNull { root ->
                val stack = navigationState.subStacks.getValue(root)
                if (remaining in 1..stack.size) {
                    root to stack[remaining - 1]
                } else {
                    remaining -= stack.size
                    null
                }
            }
        }
    }
    val target = if (progress != null) previous else null
    val selectedRoot = target?.first ?: navigationState.currentTopLevelKey
    ShellScaffold(
        navigationState,
        target?.second,
        progress,
        suiteType,
        navigationItems = { ShellNavigationItems(selectedRoot, progress, navigator, suiteType) },
        modifier = modifier,
        content = content,
    )
}

@Composable
private fun ShellNavDisplay(
    sceneState: SceneState<NavKey>,
    navigationEventState: NavigationEventState<SceneInfo<NavKey>>,
    movedBetweenTopLevels: () -> Boolean,
    backLeavesTopLevel: () -> Boolean,
) {
    val transitions = rememberScreenTransitions()
    NavDisplay(
        sceneState = sceneState,
        navigationEventState = navigationEventState,
        // Screens slide during transitions; this is what shows between them.
        modifier = Modifier.background(MaterialTheme.colorScheme.surface),
        transitionSpec = { if (movedBetweenTopLevels()) transitions.topLevel else transitions.forward },
        popTransitionSpec = { if (movedBetweenTopLevels()) transitions.topLevel else transitions.backward },
        // The back gesture scrubs the same transition as the back button.
        predictivePopTransitionSpec = { if (backLeavesTopLevel()) transitions.topLevel else transitions.backward },
    )
}

/**
 * The navigation component for the window: Material's choice (a bar in
 * compact windows and in short ones, a collapsed rail otherwise), with the
 * rail expanded from the large breakpoint, where there is room for labels.
 */
@OptIn(ExperimentalMaterial3AdaptiveApi::class)
internal fun navigationSuiteType(adaptiveInfo: WindowAdaptiveInfo): NavigationSuiteType {
    val type = NavigationSuiteScaffoldDefaults.navigationSuiteType(adaptiveInfo)
    val large = adaptiveInfo.windowSizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_LARGE_LOWER_BOUND)
    return if (type == NavigationSuiteType.WideNavigationRailCollapsed && large) {
        NavigationSuiteType.WideNavigationRailExpanded
    } else {
        type
    }
}

/**
 * Panes per Material's breakpoints: one below expanded; two from expanded,
 * the list fixed at 360dp, or 412dp from large, with a 24dp spacer. Panes
 * keep clear of a separating hinge on foldables.
 */
@OptIn(ExperimentalMaterial3AdaptiveApi::class)
internal fun paneDirective(adaptiveInfo: WindowAdaptiveInfo): PaneScaffoldDirective {
    val base = calculatePaneScaffoldDirective(adaptiveInfo)
    val sizeClass = adaptiveInfo.windowSizeClass
    val partitions = if (sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_EXPANDED_LOWER_BOUND)) 2 else 1
    val paneWidth = if (sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_LARGE_LOWER_BOUND)) 412.dp else 360.dp
    return base.copy(maxHorizontalPartitions = partitions, defaultPanePreferredWidth = paneWidth, horizontalPartitionSpacerSize = 24.dp)
}
