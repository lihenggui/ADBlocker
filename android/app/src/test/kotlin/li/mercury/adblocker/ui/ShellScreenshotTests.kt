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

import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.Posture
import androidx.compose.material3.adaptive.WindowAdaptiveInfo
import androidx.compose.material3.adaptive.navigation3.ListDetailSceneStrategy
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.input.InputModeManager
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.click
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isSelectable
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.pressKey
import androidx.compose.ui.test.swipe
import androidx.compose.ui.unit.dp
import androidx.window.core.layout.WindowSizeClass
import androidx.window.core.layout.computeWindowSizeClass
import li.mercury.adblocker.R
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.core.navigation.rememberNavigationState
import li.mercury.adblocker.core.screenshottesting.DefaultTestDevices
import li.mercury.adblocker.core.screenshottesting.DemoApp
import li.mercury.adblocker.core.screenshottesting.captureForDevice
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.PaneSurface
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.feature.activity.api.navigation.ActivityNavKey
import li.mercury.adblocker.feature.appdetail.api.navigation.AppDetailNavKey
import li.mercury.adblocker.feature.apps.api.navigation.AppsNavKey
import li.mercury.adblocker.feature.filters.api.navigation.FiltersNavKey
import li.mercury.adblocker.feature.overview.api.navigation.OverviewNavKey
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode

/**
 * The shell around the screens: the navigation bar, the collapsed or
 * expanded rail, and a list beside its detail, at each width. The screens
 * are stand-ins that need no view model; their metadata places them in panes
 * as the real ones do.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class ShellScreenshotTests {
    @get:Rule
    val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    private val devices = listOf(
        Device("phone", DefaultTestDevices.PHONE.spec, 412, 915),
        Device("foldable", DefaultTestDevices.FOLDABLE.spec, 673, 841),
        Device("tablet", DefaultTestDevices.TABLET.spec, 1280, 800),
    )

    @Test
    fun givenTheOverview_whenShownOnEachDevice_thenTheBarOrTheRailLeadsToEachDestination() {
        for (device in devices) {
            composeTestRule.captureForDevice(device.name, device.spec, "Shell") { Shell(device) }
        }
    }

    @Test
    fun givenAnAppOpenedFromTheList_whenShownOnEachDevice_thenItIsBesideTheListWhereThereIsRoom() {
        for (device in devices) {
            composeTestRule.captureForDevice(device.name, device.spec, "ShellListDetail") {
                Shell(device) {
                    navigate(AppsNavKey)
                    navigate(AppDetailNavKey(DemoApp.News.packageName))
                }
            }
        }
    }

    @Test
    fun givenTheAppListAlone_whenShownOnATablet_thenTheDetailPaneHoldsAPlaceholder() {
        val tablet = devices.last()
        composeTestRule.captureForDevice(tablet.name, tablet.spec, "ShellListPlaceholder") { Shell(tablet) { navigate(AppsNavKey) } }
    }

    @Test
    fun givenAnAppOpened_whenShownExtraLarge_thenItsRequestsAreBesideItsPage() {
        val desktop = Device("desktop", "spec:shape=Normal,width=1680,height=1000,unit=dp,dpi=320", 1680, 1000)
        composeTestRule.captureForDevice(desktop.name, desktop.spec, "ShellExtraLarge") {
            Shell(desktop) {
                navigate(AppsNavKey)
                navigate(AppDetailNavKey(DemoApp.News.packageName))
            }
        }
    }

    @Test
    @Config(qualifiers = "w1280dp-h800dp")
    fun givenTwoPanes_whenTapped_thenTheHandleSwitchesBetweenAnchors() {
        val anchorChanges = mutableListOf<Boolean>()
        val handle = showTwoPanes(anchorChanges)

        handle.performTouchInput { click() }
        handle.performTouchInput { click() }

        composeTestRule.runOnIdle { assertEquals(listOf(false, true, false), anchorChanges) }
    }

    @Test
    @Config(qualifiers = "w1280dp-h800dp")
    fun givenTwoPanes_whenDragged_thenTheHandleSettlesAtTheNextAnchor() {
        val anchorChanges = mutableListOf<Boolean>()
        val handle = showTwoPanes(anchorChanges)

        handle.performTouchInput { swipe(center, center + Offset(180.dp.toPx(), 0f)) }

        composeTestRule.runOnIdle { assertEquals(listOf(false, true), anchorChanges) }
    }

    @Test
    @Config(qualifiers = "w1280dp-h800dp")
    fun givenTwoPanes_whenActivatedThroughAccessibilityAndKeyboard_thenTheHandleSwitchesAnchors() {
        val anchorChanges = mutableListOf<Boolean>()
        lateinit var inputModeManager: InputModeManager
        val handle = showTwoPanes(anchorChanges) { inputModeManager = it }

        handle.performSemanticsAction(SemanticsActions.OnClick)
        composeTestRule.runOnIdle { inputModeManager.requestInputMode(InputMode.Keyboard) }
        handle.performSemanticsAction(SemanticsActions.RequestFocus)
        handle.assertIsFocused().performKeyInput { pressKey(Key.Enter) }

        composeTestRule.runOnIdle { assertEquals(listOf(false, true, false), anchorChanges) }
    }

    @Test
    @Config(qualifiers = "en-rUS-w412dp-h915dp")
    fun givenCompactAppDetail_whenPredictiveBackCancelsThenCommits_thenBarFollowsThePreview() {
        composeTestRule.setContent {
            Shell(devices.first()) {
                navigate(AppsNavKey)
                navigate(AppDetailNavKey(DemoApp.News.packageName))
            }
        }
        val dispatcher = composeTestRule.activity.onBackPressedDispatcher
        val nav = composeTestRule.onNode(hasText("Apps") and isSelectable())
        val detail = composeTestRule.onNodeWithTag("App detail")
        detail.assertIsDisplayed()
        nav.assertDoesNotExist()

        fun swipeBack() {
            composeTestRule.runOnUiThread {
                dispatcher.dispatchOnBackStarted(BackEventCompat(1f, 400f, 0f, BackEventCompat.EDGE_LEFT))
                dispatcher.dispatchOnBackProgressed(BackEventCompat(160f, 400f, 0.65f, BackEventCompat.EDGE_LEFT))
            }
            composeTestRule.waitForIdle()
        }

        swipeBack()
        nav.assertIsDisplayed()
        composeTestRule.runOnUiThread { dispatcher.dispatchOnBackCancelled() }
        nav.assertDoesNotExist()
        detail.assertIsDisplayed()

        swipeBack()
        composeTestRule.runOnUiThread { dispatcher.onBackPressed() }
        nav.assertIsDisplayed()
        detail.assertDoesNotExist()
    }

    private fun showTwoPanes(
        anchorChanges: MutableList<Boolean>,
        onInputModeManager: (InputModeManager) -> Unit = {},
    ): SemanticsNodeInteraction {
        composeTestRule.setContent {
            val inputModeManager = LocalInputModeManager.current
            SideEffect { onInputModeManager(inputModeManager) }
            Shell(devices.last(), onListPaneWide = anchorChanges::add) {
                navigate(AppsNavKey)
                navigate(AppDetailNavKey(DemoApp.News.packageName))
            }
        }
        return composeTestRule.onNodeWithContentDescription(composeTestRule.activity.getString(R.string.pane_drag_handle))
    }

    private class Device(val name: String, val spec: String, val width: Int, val height: Int)

    @OptIn(ExperimentalMaterial3AdaptiveApi::class)
    @Composable
    private fun Shell(device: Device, onListPaneWide: (Boolean) -> Unit = {}, moves: Navigator.() -> Unit = {}) {
        val topLevelKeys = remember { TOP_LEVEL_DESTINATIONS.map { it.key }.toSet() }
        val state = rememberNavigationState(startKey = OverviewNavKey, topLevelKeys = topLevelKeys)
        val navigator = remember(state) { Navigator(state).apply(moves) }
        ADBlockerShell(
            navigationState = state,
            navigator = navigator,
            // As the window reports it: the lower bounds of its size classes.
            adaptiveInfo = WindowAdaptiveInfo(WindowSizeClass.BREAKPOINTS_V2.computeWindowSizeClass(device.width, device.height), Posture()),
            onListPaneWide = onListPaneWide,
            sidePane = { StandIn("Requests", DemoApp.entries.take(4)) },
        ) {
            entry<OverviewNavKey> { StandIn("ADBlocker", DemoApp.entries.take(3)) }
            entry<AppsNavKey>(
                metadata = ListDetailSceneStrategy.listPane(detailPlaceholder = { PaneSurface { Placeholder(AdbIcons.Apps, "Choose an app") } }),
            ) { StandIn("Apps", DemoApp.entries.take(8)) }
            entry<AppDetailNavKey>(metadata = ListDetailSceneStrategy.detailPane()) {
                StandIn("Daily Brief", DemoApp.entries.drop(2).take(3), Modifier.testTag("App detail"))
            }
            entry<FiltersNavKey> { StandIn("Filters", emptyList()) }
            entry<ActivityNavKey>(metadata = ListDetailSceneStrategy.listPane()) { StandIn("Activity", emptyList()) }
        }
    }

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun StandIn(title: String, apps: List<DemoApp>, modifier: Modifier = Modifier) {
        Scaffold(modifier = modifier, topBar = { TopAppBar(title = { Text(title) }) }) { padding ->
            LazyColumn(Modifier.padding(padding).fillMaxSize(), contentPadding = PaddingValues(horizontal = 16.dp)) {
                item { SectionHeader(title) }
                item {
                    SegmentedGroup(apps.size) { index ->
                        val app = apps[index].installedApp
                        SegmentedItem(index = index, count = apps.size, headline = app.label, leading = { AppIcon(app, app.label) }, onClick = {})
                    }
                }
            }
        }
    }
}
