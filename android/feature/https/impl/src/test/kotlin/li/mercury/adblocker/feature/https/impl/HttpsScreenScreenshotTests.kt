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

package li.mercury.adblocker.feature.https.impl

import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
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
class HttpsScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice("phone", DefaultTestDevices.PHONE.spec, name, body = body)
    }

    private fun captureChinese(name: String, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, name, body = body)
    }

    private val confirmation = HttpsUiState(step = HttpsStep.Confirm)

    private val result: HttpsUiState
        get() = confirmation.copy(
            step = HttpsStep.Result,
            results = ReadState.Available(
                DemoData.entries.filter { it.isFiltered }.map { entry ->
                    AppEntry(entry.app, entry.filter!!.copy(httpLevel = FilterLevel.Full, level = FilterLevel.Full, levelReason = null))
                },
            ),
        )

    @Test
    fun givenFilteringEnabledWithUnavailableAppResults_whenShown_thenItStillOffersDone() {
        capturePhone("HttpsResultsUnavailable") { Https(result.copy(results = ReadState.Unavailable(LoadFailure.TimedOut))) }
    }

    @Test
    fun givenTheSetupOpened_whenShownOnEachDevice_thenSaysWhatItCosts() {
        composeTestRule.captureMultiDevice("HttpsImpact") { Https(HttpsUiState()) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheSetupOpened_whenShownInChinese_thenMatchesGolden() {
        captureChinese("HttpsImpact") { Https(HttpsUiState()) }
    }

    @Test
    fun givenTheCertificateNotSaved_whenShown_thenOffersToSaveIt() {
        capturePhone("HttpsInstall") { Https(HttpsUiState(step = HttpsStep.Install)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheCertificateNotSaved_whenShownInChinese_thenMatchesGolden() {
        captureChinese("HttpsInstall") { Https(HttpsUiState(step = HttpsStep.Install)) }
    }

    @Test
    fun givenBackFromTheSettingsWithoutIt_whenShown_thenSaysItIsNotInstalledYet() {
        capturePhone("HttpsInstallNotFound") { Https(HttpsUiState(step = HttpsStep.Install, saved = true, notFound = true)) }
    }

    @Test
    fun givenTheCertificateInstalled_whenConfirmed_thenOneActionEnablesUrlFiltering() {
        capturePhone("HttpsApps") { Https(confirmation) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheCertificateInstalled_whenConfirmedInChinese_thenMatchesGolden() {
        captureChinese("HttpsApps") { Https(confirmation) }
    }

    @Test
    fun givenItIsOn_whenShown_thenSaysWhatEachAppGetsAndHowToUndoIt() {
        capturePhone("HttpsResult") { Https(result) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenItIsOn_whenShownInChinese_thenMatchesGolden() {
        captureChinese("HttpsResult") { Https(result) }
    }

    @Composable
    private fun Https(state: HttpsUiState) {
        HttpsScreen(
            state = state,
            onBack = {},
            onNext = {},
            onPrevious = {},
            onSave = {},
            onOpenSettings = {},
            onTurnOn = {},
        )
    }
}
