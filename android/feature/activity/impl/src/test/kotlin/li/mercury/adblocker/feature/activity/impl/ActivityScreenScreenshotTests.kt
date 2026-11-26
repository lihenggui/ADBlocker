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

package li.mercury.adblocker.feature.activity.impl

import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.test.assertHasClickAction
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.Verdict
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

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class ActivityScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private val state
        get() = ActivityUiState(
            events = DemoData.requests,
            recording = RecordingStatus.Stopped,
            apps = ReadState.Available(
                ActivityApps(
                    byKey = DemoData.entries.associate { it.appKey to it.app },
                    filtered = DemoData.entries.filter { it.isFiltered }.map { it.app },
                ),
            ),
            idle = ReadState.Available(false),
        )

    @Test
    fun givenMetadataUnavailable_whenRecordingIsShown_thenRequestsRemainVisible() {
        val unavailable = ReadState.Unavailable(LoadFailure.TimedOut)
        capturePhone("ActivityMetadataUnavailable") {
            Activity(state.copy(recording = RecordingStatus.Recording, apps = unavailable, idle = unavailable))
        }
    }

    @Test
    fun givenRequests_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Activity") { Activity(state) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRequests_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Activity") { Activity(state) }
    }

    @Test
    fun givenRecording_whenShownInDarkTheme_thenOffersToStop() {
        capturePhone("Activity", darkMode = true) { Activity(state.copy(recording = RecordingStatus.Recording)) }
        composeTestRule.onNodeWithContentDescription(composeTestRule.activity.getString(R.string.feature_activity_stop_recording)).assertHasClickAction()
    }

    @Test
    fun givenNothingYet_whenShown_thenOffersToStartRecording() {
        capturePhone("ActivityEmpty") { Activity(ActivityUiState()) }
        composeTestRule.onNodeWithContentDescription(composeTestRule.activity.getString(R.string.feature_activity_start_recording)).assertHasClickAction()
    }

    @Test
    fun givenBlockedRequest_whenOpened_thenNamesTheRuleAndOffersToTrustTheSite() {
        val event = DemoData.requests.first()
        val rule = RuleCheckResult(blocked = true, rule = "||adnet.example^\$third-party", source = RuleSource.List(DemoData.EASYLIST))
        capturePhone("Request") {
            RequestContent(event.copy(reason = null), RequestUiState(request = event, app = DemoApp.News.installedApp, rule = rule), onTrust = {}, onBlock = {}, onBlockInApp = {}, onForget = {})
        }
    }

    @Test
    fun givenAllowedRequest_whenOpened_thenOffersToBlockItInItsAppOrInAllApps() {
        capturePhone("RequestAllowed") { AllowedRequest() }
        composeTestRule.onNodeWithText(composeTestRule.activity.getString(R.string.feature_activity_block_in_all_apps)).assertHasClickAction()
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAllowedRequestInChinese_whenOpened_thenBothBlockActionsFit() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "RequestAllowed") { AllowedRequest() }
    }

    @Composable
    private fun AllowedRequest() {
        val event = DemoData.requests.first { it.verdict == Verdict.Allowed && it.packageName == DemoApp.News.packageName }
        RequestContent(event, RequestUiState(request = event, app = DemoApp.News.installedApp), onTrust = {}, onBlock = {}, onBlockInApp = {}, onForget = {})
    }

    @Test
    fun givenBankRequest_whenOpened_thenSaysWhyItWasNotDecrypted() {
        val event = DemoData.requests.first { it.packageName == DemoApp.Bank.packageName }
        capturePhone("RequestPassed") {
            RequestContent(event, RequestUiState(request = event, app = DemoApp.Bank.installedApp), onTrust = {}, onBlock = {}, onBlockInApp = {}, onForget = {})
        }
    }

    @Test
    fun givenHostBlocked_whenOpened_thenSaysTheConnectionWasRefusedWhole() {
        val event = DemoData.requests.first { it.verdict == Verdict.HostBlocked }
        capturePhone("RequestHostBlocked") {
            RequestContent(event, RequestUiState(request = event, app = DemoApp.Puzzle.installedApp), onTrust = {}, onBlock = {}, onBlockInApp = {}, onForget = {})
        }
    }

    @Test
    fun givenAnAppsRequests_whenShownBesideItsPage_thenListsThem() {
        val events = DemoData.requests.filter { it.packageName == DemoApp.News.packageName }
        capturePhone("AppRequestsPane") { AppRequestsPane(AppRequestsUiState(app = DemoApp.News.installedApp, events = events), onOpenActivity = {}) }
    }

    @Composable
    private fun Activity(state: ActivityUiState) {
        ActivityScreen(state = state, selectedId = null, onVerdict = {}, onApp = {}, onStartRecording = {}, onStopRecording = {}, onOpenRequest = {}, onOpenSettings = {})
    }
}
