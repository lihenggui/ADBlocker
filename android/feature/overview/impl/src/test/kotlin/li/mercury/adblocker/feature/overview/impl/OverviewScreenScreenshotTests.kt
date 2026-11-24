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

package li.mercury.adblocker.feature.overview.impl

import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
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
import java.time.Instant

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class OverviewScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    @Test
    fun givenFiltering_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Overview") { Overview(ready()) }
    }

    @Test
    fun givenFiltering_whenShownInDarkTheme_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_dark", DefaultTestDevices.PHONE.spec, "Overview", darkMode = true) { Overview(ready()) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenFiltering_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Overview") { Overview(ready()) }
    }

    @Test
    fun givenPausedForAnHour_whenShown_thenSaysUntilWhen() {
        val status = DemoData.status.copy(paused = true, resumeAt = clock.now.plusSeconds(3_600))
        capturePhone("OverviewPaused") { Overview(ready().copy(status = status)) }
        composeTestRule.onNodeWithText("Resumes in 1 hour").assertIsDisplayed()
    }

    @Test
    fun givenServiceStopped_whenShown_thenOffersToStartIt() {
        val status = DemoData.status.copy(running = false, pid = null, startedAt = null)
        capturePhone("OverviewStopped") { Overview(ready().copy(status = status)) }
    }

    @Test
    fun givenTurnedOffAfterABootThatDidNotFinish_whenShown_thenSaysWhenAndOffersToTurnItOnAgain() {
        val status = DemoData.status.copy(
            running = false,
            pid = null,
            startedAt = null,
            bootGuard = Instant.parse("2026-11-18T07:42:00Z"),
        )
        capturePhone("OverviewTurnedOff") { Overview(ready().copy(status = status)) }
    }

    @Test
    fun givenNoRoot_whenShown_thenSaysWhatIsMissing() {
        capturePhone("OverviewNoRoot") { Overview(OverviewUiState.Failed(LoadFailure.NoRoot)) }
    }

    @Test
    fun givenRootModeNotSetUp_whenShown_thenSaysWhereToSetItUp() {
        capturePhone("OverviewNotSetUp") { Overview(OverviewUiState.Failed(LoadFailure.ModuleMissing)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRootModeNotSetUp_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewNotSetUp") {
            Overview(OverviewUiState.Failed(LoadFailure.ModuleMissing))
        }
    }

    @Test
    fun givenAnOlderRootMode_whenACommandIsRejected_thenSaysItNeedsAnUpdate() {
        capturePhone("OverviewUpdateNeeded") { Overview(OverviewUiState.Failed(LoadFailure.ModuleOutdated)) }
    }

    @Test
    fun givenAnOlderModuleItsManagerHoldsForARestart_whenShown_thenSaysToRestart() {
        capturePhone("OverviewRestartNeeded") { Overview(OverviewUiState.RestartNeeded) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAnOlderModuleItsManagerHoldsForARestart_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewRestartNeeded") { Overview(OverviewUiState.RestartNeeded) }
    }

    @Test
    fun givenRootModeUpdating_whenShown_thenSaysSo() {
        capturePhone("OverviewUpdating") { Overview(OverviewUiState.Updating) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRootModeUpdating_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewUpdating") { Overview(OverviewUiState.Updating) }
    }

    @Test
    fun givenTheVpn_whenFiltering_thenSaysTheMethodAndHowFarHttpsIsSetUp() {
        capturePhone("OverviewVpn") { Overview(vpn()) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpn_whenFilteringInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewVpn") { Overview(vpn()) }
    }

    @Test
    fun givenListsUpdatedFiveHoursBeforeTheDemoTime_whenShown_thenShowsTheirAge() {
        val updated = Instant.parse("2026-11-19T10:00:00Z")
        composeTestRule.captureForDevice("tablet", DefaultTestDevices.TABLET.spec, "OverviewVpn") {
            Overview(vpn().copy(listsUpdated = updated))
        }
        composeTestRule.onNodeWithText("updated 5 hr. ago", substring = true).assertIsDisplayed()
    }

    @Test
    fun givenAnotherVpnTookOver_whenShown_thenSaysTheAppsConnectUnfilteredAndOffersToReconnect() {
        capturePhone("OverviewVpnRevoked") { Overview(vpn(VpnStatus(state = VpnState.Revoked))) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAnotherVpnTookOver_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewVpnRevoked") { Overview(vpn(VpnStatus(state = VpnState.Revoked))) }
    }

    @Test
    fun givenTheVpnOffWithLockdown_whenShown_thenSaysTheAppsCannotConnect() {
        capturePhone("OverviewVpnLockdownOff") { Overview(vpn(VpnStatus(state = VpnState.Off, alwaysOn = true, lockdown = true))) }
    }

    @Test
    fun givenTheVpnStarting_whenShown_thenSaysItIsConnecting() {
        capturePhone("OverviewVpnConnecting") { Overview(vpn(VpnStatus(state = VpnState.Starting))) }
    }

    @Test
    fun givenLockdownOn_whenShownTheFirstTime_thenSaysWhatItMeans() {
        capturePhone("OverviewVpnLockdown") {
            Overview(vpn(VpnStatus(state = VpnState.On, alwaysOn = true, lockdown = true)).copy(notice = OverviewNotice.Lockdown))
        }
    }

    @Test
    fun givenRootGrantedSinceTheVpnWasChosen_whenShown_thenOffersRootModeOnce() {
        capturePhone("OverviewVpnRoot") { Overview(vpn().copy(notice = OverviewNotice.RootAvailable)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRootGrantedSinceTheVpnWasChosen_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewVpnRoot") {
            Overview(vpn().copy(notice = OverviewNotice.RootAvailable))
        }
    }

    @Test
    fun givenHttpsSetUp_whenShown_thenSaysHowManyAppsAreDecrypted() {
        // On a tablet, where the status is beside the counts rather than below them.
        composeTestRule.captureForDevice("tablet", DefaultTestDevices.TABLET.spec, "OverviewVpnHttps") {
            Overview(vpn().copy(https = HttpsSetup(certificateInstalled = true, urlFiltering = true, decryptedApps = 1)))
        }
    }

    @Test
    fun givenAnEnabledListNeverDownloaded_whenShown_thenSaysItIsNotDownloadedYet() {
        capturePhone("OverviewListsPending") { Overview(listsMissing(failed = false)) }
        composeTestRule.onNodeWithText("1 list not downloaded yet").assertIsDisplayed()
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAnEnabledListNeverDownloaded_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewListsPending") { Overview(listsMissing(failed = false)) }
    }

    @Test
    fun givenListsWhoseFirstDownloadFailed_whenShown_thenSaysTheDownloadFailed() {
        capturePhone("OverviewListsFailed") { Overview(listsMissing(failed = true)) }
        composeTestRule.onNodeWithText("2 lists failed to download").assertIsDisplayed()
    }

    @Test
    fun givenListsWhoseFirstDownloadFailed_whenShownInDarkTheme_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_dark", DefaultTestDevices.PHONE.spec, "OverviewListsFailed", darkMode = true) { Overview(listsMissing(failed = true)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenListsWhoseFirstDownloadFailed_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OverviewListsFailed") { Overview(listsMissing(failed = true)) }
    }

    private fun listsMissing(failed: Boolean) = ready().copy(
        missingLists = MissingLists(count = if (failed) 2 else 1, failed = failed),
        notice = OverviewNotice.ListsMissing,
    )

    /** The VPN defaults to connected with URL filtering on and no certificate installed yet. */
    private fun vpn(status: VpnStatus = VpnStatus(state = VpnState.On)) = ready().copy(
        method = FilterMethod.Vpn,
        vpn = status,
        https = HttpsSetup(certificateInstalled = false, urlFiltering = true, decryptedApps = 0),
    )

    @Test
    fun givenAnInterruptedVpn_whenOverviewIsShown_thenOffersBatteryExemption() {
        capturePhone("OverviewVpnBattery") {
            Overview(vpn(VpnStatus(state = VpnState.Off)).copy(notice = OverviewNotice.BatteryExemption))
        }
    }

    private fun ready() = OverviewUiState.Ready(
        status = DemoData.status,
        filteredApps = DemoData.filtered.size,
        stats = DemoData.stats,
        topApps = DemoData.topApps,
        enabledLists = DemoData.lists.count { it.enabled },
        listsUpdated = DemoData.status.listsUpdated,
    )

    private fun capturePhone(name: String, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice("phone", DefaultTestDevices.PHONE.spec, name, body = body)
    }

    @Composable
    private fun Overview(state: OverviewUiState) {
        OverviewScreen(
            state = state,
            onPause = {},
            onResume = {},
            onStartService = {},
            onRetry = {},
            onOpenApp = {},
            onOpenFilters = {},
            onOpenSettings = {},
        )
    }
}
