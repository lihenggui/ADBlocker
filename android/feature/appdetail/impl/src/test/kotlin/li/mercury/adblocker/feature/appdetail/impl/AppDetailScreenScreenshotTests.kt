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

package li.mercury.adblocker.feature.appdetail.impl

import androidx.activity.ComponentActivity
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
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
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.screenshottesting.DefaultTestDevices
import li.mercury.adblocker.core.screenshottesting.DemoApp
import li.mercury.adblocker.core.screenshottesting.DemoClockRule
import li.mercury.adblocker.core.screenshottesting.DemoData
import li.mercury.adblocker.core.screenshottesting.captureForDevice
import li.mercury.adblocker.core.screenshottesting.captureMultiDevice
import li.mercury.adblocker.core.ui.LocalSnackbarHostState
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
class AppDetailScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private fun state(app: DemoApp) = AppDetailUiState(
        loading = false,
        entry = DemoData.entry(app),
        counts = ReadState.Available(DemoData.appCounts[app.packageName]),
        pinned = ReadState.Available(DemoData.pinned.filter { it.packageName == app.packageName }),
    )

    @Test
    fun givenBothSecondaryReadsFail_whenAnAppIsShown_thenBothSectionsReportUnavailability() {
        val unavailable = ReadState.Unavailable(LoadFailure.TimedOut)
        capturePhone("AppDetailReadsUnavailable") { Detail(state(DemoApp.News).copy(counts = unavailable, pinned = unavailable)) }
    }

    @Test
    fun givenFilteredApp_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("AppDetail") { Detail(state(DemoApp.News)) }
    }

    @Test
    fun givenFilteredApp_whenShownInDarkTheme_thenMatchesGolden() {
        capturePhone("AppDetail", darkMode = true) { Detail(state(DemoApp.News)) }
    }

    @Test
    fun givenBankFilteredByDomain_whenShown_thenExplainsWhy() {
        capturePhone("AppDetailBank") { Detail(state(DemoApp.Bank).copy(pinnedExpanded = true)) }
    }

    @Test
    fun givenDecryptionOff_whenShown_thenSaysWhyTheAppIsFilteredByDomainOnly() {
        val news = state(DemoApp.News)
        val filter = news.entry!!.filter!!.copy(httpLevel = FilterLevel.Hosts, level = FilterLevel.Hosts, levelReason = LevelReason.UrlFilteringOff)
        capturePhone("AppDetailDecryptionOff") { Detail(news.copy(entry = AppEntry(news.entry.app, filter))) }
    }

    /** Without root: [app] in [mode], at [level] for [reason], with the certificate [installed] or not. */
    private fun vpn(app: DemoApp, mode: FilterLevel, level: FilterLevel, reason: LevelReason?, installed: Boolean): AppDetailUiState {
        val detail = state(app)
        val filter = detail.entry!!.filter!!.copy(httpLevel = mode, level = level, levelReason = reason)
        return detail.copy(entry = AppEntry(detail.entry.app, filter), method = FilterMethod.Vpn, certificateInstalled = installed, pinned = ReadState.Available(emptyList()))
    }

    @Test
    fun givenTheVpnAndUrlFilteringOff_whenCertificateIsMissing_thenOffersSetup() {
        capturePhone("AppDetailVpnDomainOnly") { Detail(vpn(DemoApp.News, FilterLevel.Hosts, FilterLevel.Hosts, LevelReason.UrlFilteringOff, installed = false)) }
        composeTestRule.onNodeWithText(composeTestRule.activity.getString(R.string.feature_appdetail_set_up_https)).assertIsDisplayed()
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpnAndDomainOnly_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "AppDetailVpnDomainOnly") {
            Detail(vpn(DemoApp.News, FilterLevel.Hosts, FilterLevel.Hosts, LevelReason.UrlFilteringOff, installed = false))
        }
    }

    @Test
    fun givenTheVpnAndFullWithoutTheCertificate_whenShown_thenLinksToTheSetup() {
        capturePhone("AppDetailVpnNoCertificate") { Detail(vpn(DemoApp.Video, FilterLevel.Full, FilterLevel.Hosts, LevelReason.CaNotInstalled, installed = false)) }
    }

    @Test
    fun givenTheVpnAndTheCertificateWithDecryptionOff_whenShown_thenSaysDecryptionIsOff() {
        capturePhone("AppDetailVpnDecryptionOff") { Detail(vpn(DemoApp.Video, FilterLevel.Hosts, FilterLevel.Hosts, LevelReason.UrlFilteringOff, installed = true)) }
    }

    @Test
    fun givenTheVpnAndAReasonThisAppDoesNotKnow_whenShown_thenSaysDomainOnlyForAReasonUnknown() {
        capturePhone("AppDetailVpnUnknown") { Detail(vpn(DemoApp.Video, FilterLevel.Full, FilterLevel.Hosts, LevelReason.Unknown, installed = true)) }
    }

    @Test
    fun givenTheVpnAndABrowserDecrypted_whenShown_thenNeedsNoNote() {
        capturePhone("AppDetailVpnFull") { Detail(vpn(DemoApp.Browser, FilterLevel.Full, FilterLevel.Full, null, installed = true)) }
    }

    @Test
    fun givenAppNotFiltered_whenShown_thenOffersToFilterIt() {
        capturePhone("AppDetailOff") { Detail(state(DemoApp.Weather)) }
    }

    @Test
    fun givenASharedGroup_whenOpened_thenAllKnownMembersAreShown() {
        val news = state(DemoApp.News)
        val app = news.entry!!.app.copy(
            packageName = "@com.example.suite",
            sharedProfileSerial = 37,
            sharedMembers = listOf(SharedAppMember(news.entry.packageName, news.entry.app.label), SharedAppMember("com.example.hidden", "com.example.hidden")),
        )
        val selected = FilteredApp(app.packageName, FilterLevel.Hosts, null, profileSerial = 37)
        capturePhone("AppDetailSharedGroup") { Detail(news.copy(entry = AppEntry(app, selected), pinned = ReadState.Available(emptyList()))) }
        composeTestRule.onNodeWithText("com.example.hidden", substring = true).assertIsDisplayed()
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenASavedSharedUidApp_whenDetailsAreShown_thenRemovalIsOfferedInChinese() {
        val app = DemoData.entry(DemoApp.News).app.copy(icon = null)
        val selection = FilteredApp(app.packageName, FilterLevel.Full, null, level = null, unavailableReason = AppUnavailableReason.SharedUid)
        val actions = mutableListOf<Boolean>()
        composeTestRule.setContent {
            ADBlockerTheme(useDynamicColor = false) {
                CompositionLocalProvider(LocalSnackbarHostState provides remember { SnackbarHostState() }) {
                    Detail(AppDetailUiState(loading = false, entry = AppEntry(app, selection))) { actions += it }
                }
            }
        }
        composeTestRule.onNodeWithText("此应用与其他应用共用 Android UID，无法在 Root 模式下单独过滤。").assertIsDisplayed()
        composeTestRule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Switch)).assertDoesNotExist()
        composeTestRule.onNodeWithText("移除选择").performClick()
        composeTestRule.runOnIdle { assertEquals(listOf(false), actions) }
    }

    @Composable
    private fun Detail(state: AppDetailUiState, onFilterChange: (Boolean) -> Unit = {}) {
        AppDetailScreen(
            state = state,
            onBack = {},
            onFilterChange = onFilterChange,
            onPause = {},
            onResume = {},
            onTogglePins = {},
            onForget = {},
            onOpenActivity = {},
            onRetry = {},
        )
    }
}
