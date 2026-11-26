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

import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import li.mercury.adblocker.core.designsystem.theme.ADBlockerTheme
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.screenshottesting.DefaultTestDevices
import li.mercury.adblocker.core.screenshottesting.DemoApp
import li.mercury.adblocker.core.screenshottesting.DemoClockRule
import li.mercury.adblocker.core.screenshottesting.DemoData
import li.mercury.adblocker.core.screenshottesting.captureForDevice
import li.mercury.adblocker.core.screenshottesting.captureMultiDevice
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import kotlin.test.assertEquals

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class AppsScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private val state get() = AppsUiState(loading = false, entries = DemoData.entries, counts = ReadState.Available(DemoData.appCounts))

    @Test
    fun givenBlockCountsUnavailable_whenAppsAreShown_thenFilteringModesRemainVisible() {
        capturePhone("AppsCountsUnavailable") { Apps(state.copy(counts = ReadState.Unavailable(LoadFailure.TimedOut))) }
    }

    @Test
    fun givenFilteredApps_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Apps") { Apps(state) }
    }

    @Test
    fun givenFilteredApps_whenShownInDarkTheme_thenMatchesGolden() {
        capturePhone("Apps", darkMode = true) { Apps(state) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenFilteredApps_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Apps") { Apps(state) }
    }

    @Test
    fun givenAllApps_whenShown_thenBanksSayTheyAreFilteredByDomain() {
        capturePhone("AppsAll") { Apps(state.copy(filter = AppFilter.All)) }
    }

    @Test
    fun givenALongAppName_whenShownOnPhone_thenTheRowEllipsizesAndKeepsTheFullAccessibleName() {
        val label = "Example News and Weather for the Pacific Northwest"
        val entry = DemoData.entry(DemoApp.News)
        capturePhone("AppsLongName") { Apps(state.copy(entries = listOf(entry.copy(app = entry.app.copy(label = label))))) }
        composeTestRule.onNodeWithText(label).assertIsDisplayed()
    }

    @Test
    fun givenNoFilteredApps_whenBrowsingAppsFromTheEmptyState_thenItSelectsAllApps() {
        val filters = mutableListOf<AppFilter>()
        val available = DemoData.entry(DemoApp.News).copy(filter = null)
        capturePhone("AppsEmptyFiltered") { Apps(state.copy(entries = listOf(available)), onFilter = filters::add) }
        composeTestRule.onNodeWithText("Browse apps").assertIsDisplayed().performClick()
        composeTestRule.runOnIdle { assertEquals(listOf(AppFilter.All), filters) }
    }

    @Test
    fun givenTheSystemWithholdsTheAppList_whenShownOnEachDevice_thenItAsksToAllowReadingIt() {
        composeTestRule.captureMultiDevice("AppsListWithheld") { Apps(withheld) }
    }

    @Test
    fun givenTheSystemWithholdsTheAppList_whenShownInDarkTheme_thenItAsksToAllowReadingIt() {
        capturePhone("AppsListWithheld", darkMode = true) { Apps(withheld) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheSystemWithholdsTheAppList_whenShownInChinese_thenItAsksToAllowReadingIt() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "AppsListWithheld") { Apps(withheld) }
    }

    private val withheld get() = AppsUiState(loading = false, listWithheld = true, counts = ReadState.Available(emptyMap()))

    @Test
    fun givenAllApps_whenListed_thenIncludedAndExcludedAppsHaveExplicitActions() {
        val one = DemoData.entry(DemoApp.News)
        val two = DemoData.entry(DemoApp.Browser)
        capturePhone("AppsScopeAll") {
            Apps(state.copy(entries = listOf(AppEntry(one.app, one.filter, inAllAppsScope = true), AppEntry(two.app, null, inAllAppsScope = true, excluded = true)), filter = AppFilter.All))
        }
    }

    @Test
    fun givenDifferentEffectiveHttpAndHttpsModes_whenShown_thenBothAppearInTheStatus() {
        val app = DemoData.entry(DemoApp.News).app.copy(icon = null)
        val selection = FilteredApp(app.packageName, FilterLevel.Full, null, level = FilterLevel.Hosts)
        composeTestRule.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                AppRow(AppEntry(app, selection), index = 0, count = 1, counts = ReadState.Available(null), selected = false, onToggle = { _, _ -> }, onClick = {})
            }
        }
        composeTestRule.onNodeWithText("HTTP requests and HTTPS domains · 0 blocked today").assertIsDisplayed()
    }

    @Test
    fun givenAHiddenSharedMember_whenFoundAndSwitchedOn_thenTheGroupIsSelectedDirectly() {
        val member = SharedAppMember("com.example.hidden", "com.example.hidden")
        val app = DemoData.entry(DemoApp.News).app.copy(
            packageName = "@com.example.suite",
            icon = null,
            sharedProfileSerial = 37,
            sharedMembers = listOf(SharedAppMember("com.example.news", "News"), member),
        )
        val match = matchingAppRows(listOf(AppEntry(app, null)), member.packageName).single()
        val actions = mutableListOf<Pair<String, Boolean>>()
        assertEquals(member, match.member)
        composeTestRule.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                AppRow(
                    match.entry,
                    index = 0,
                    count = 1,
                    counts = ReadState.Available(null),
                    selected = false,
                    onToggle = { entry, on -> actions += entry.appKey to on },
                    onClick = {},
                    member = match.member,
                )
            }
        }
        composeTestRule.onNodeWithText("Apps filtered together · News, \u2066com.example.hidden\u2069").assertIsDisplayed()
        composeTestRule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Switch)).performClick()
        composeTestRule.runOnIdle { assertEquals(listOf("37:@com.example.suite" to true), actions) }
    }

    @Test
    fun givenASavedSharedUidApp_whenShown_thenRemoveSelectionIsOfferedWithoutASwitch() {
        val app = DemoData.entry(DemoApp.News).app.copy(icon = null)
        val selection = FilteredApp(app.packageName, FilterLevel.Full, null, level = null, unavailableReason = AppUnavailableReason.SharedUid)
        val actions = mutableListOf<Boolean>()
        composeTestRule.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                AppRow(
                    entry = AppEntry(app, selection),
                    index = 0,
                    count = 1,
                    counts = ReadState.Available(null),
                    selected = false,
                    onToggle = { _, enabled -> actions += enabled },
                    onClick = {},
                )
            }
        }
        composeTestRule.onNodeWithText("Shared Android UID, can’t filter individually").assertIsDisplayed()
        composeTestRule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Switch)).assertDoesNotExist()
        composeTestRule.onNodeWithText("Remove selection").performClick()
        composeTestRule.runOnIdle { assertEquals(listOf(false), actions) }
    }

    @Composable
    private fun Apps(state: AppsUiState, onFilter: (AppFilter) -> Unit = {}) {
        AppsScreen(state = state, selectedPackage = null, onFilter = onFilter, onToggle = { _, _ -> }, onRetry = {}, onOpenApp = {}, onOpenSettings = {})
    }
}
