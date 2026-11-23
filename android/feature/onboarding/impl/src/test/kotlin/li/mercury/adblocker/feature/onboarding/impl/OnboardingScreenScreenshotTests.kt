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

package li.mercury.adblocker.feature.onboarding.impl

import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.model.DeviceCheck
import li.mercury.adblocker.core.model.DeviceCheckKind
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.LoadFailure
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
class OnboardingScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private val passed = mapOf(
        DeviceCheckKind.Root to DeviceCheck(DeviceCheckKind.Root, passed = true, detail = "KernelSU"),
        DeviceCheckKind.AndroidVersion to DeviceCheck(DeviceCheckKind.AndroidVersion, passed = true, detail = "Android 16"),
        DeviceCheckKind.Kernel to DeviceCheck(DeviceCheckKind.Kernel, passed = true, detail = "6.6.30-android15"),
        DeviceCheckKind.Architecture to DeviceCheck(DeviceCheckKind.Architecture, passed = true, detail = "arm64-v8a"),
    )

    /** Root missing: the kernel cannot be read without it. */
    private val noRoot = passed +
        (DeviceCheckKind.Root to DeviceCheck(DeviceCheckKind.Root, passed = false, detail = null)) +
        (DeviceCheckKind.Kernel to DeviceCheck(DeviceCheckKind.Kernel, passed = false, detail = null))

    private fun vpn(step: OnboardingStep) = OnboardingUiState(
        step = step,
        checks = noRoot,
        method = FilterMethod.Vpn,
        picks = setOf(DemoApp.Browser.packageName),
        lists = FilterCatalog.setupLists("CN"),
    )

    private fun vpnDone() = vpn(OnboardingStep.Install).copy(
        notifications = true,
        install = InstallState.Done(active = true, apps = 5, rules = 141_207),
    )

    private val apps get() = listOf(DemoApp.Browser, DemoApp.News, DemoApp.Puzzle, DemoApp.Video, DemoApp.Weather, DemoApp.Bank, DemoApp.Wallet).map { it.installedApp }

    @Test
    fun givenWelcome_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Onboarding") { Onboarding(OnboardingUiState()) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenWelcome_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Onboarding") { Onboarding(OnboardingUiState()) }
    }

    @Test
    fun givenChecksPassed_whenShown_thenListsThem() {
        capturePhone("OnboardingCheck") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = passed)) }
    }

    @Test
    fun givenKernelConfigUnavailable_whenChecked_thenSaysWhenItWillBeChecked() {
        val checks = passed + (DeviceCheckKind.Kernel to DeviceCheck(DeviceCheckKind.Kernel, passed = true, detail = null))
        capturePhone("OnboardingCheckKernelUnverified") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = checks)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenKernelFeaturesMissing_whenCheckedInChinese_thenOffersTheVpn() {
        val checks = passed + (DeviceCheckKind.Kernel to DeviceCheck(DeviceCheckKind.Kernel, passed = false, detail = "6.6.30-android15"))
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OnboardingCheckKernelUnsupported") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = checks))
        }
    }

    @Test
    fun givenNoRootAndAProcessorNeitherMethodRunsOn_whenChecked_thenSaysWhatIsMissing() {
        // Without root the kernel cannot be read either.
        val failed = noRoot + (DeviceCheckKind.Architecture to DeviceCheck(DeviceCheckKind.Architecture, passed = false, detail = "armeabi-v7a"))
        capturePhone("OnboardingCheckFailed") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = failed)) }
    }

    @Test
    fun givenNoRoot_whenChecked_thenOffersTheVpnAndToCheckAgain() {
        capturePhone("OnboardingCheckNoRoot") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = noRoot)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenNoRoot_whenCheckedInChinese_thenOffersTheVpn() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OnboardingCheckNoRoot") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = noRoot))
        }
    }

    @Test
    fun givenRootOnAnAndroidTooOldForTheModule_whenChecked_thenOffersTheVpn() {
        val old = passed + (DeviceCheckKind.AndroidVersion to DeviceCheck(DeviceCheckKind.AndroidVersion, passed = false, detail = "Android 11"))
        capturePhone("OnboardingCheckUnsupported") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = old)) }
    }

    @Test
    fun givenCertificateStep_whenShown_thenAsksForAcknowledgement() {
        capturePhone("OnboardingCertificate") { Onboarding(OnboardingUiState(step = OnboardingStep.About)) }
    }

    @Test
    fun givenTheVpn_whenItsStepIsShown_thenSaysWhatItDoesWithoutAskingToAcknowledge() {
        capturePhone("OnboardingVpn") { Onboarding(vpn(OnboardingStep.About)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpn_whenItsStepIsShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OnboardingVpn") { Onboarding(vpn(OnboardingStep.About)) }
    }

    @Test
    fun givenTheVpn_whenStarting_thenListsTheGrantsTheSystemAsksFor() {
        capturePhone("OnboardingVpnStart") { Onboarding(vpn(OnboardingStep.Install)) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpn_whenStartingInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OnboardingVpnStart") { Onboarding(vpn(OnboardingStep.Install)) }
    }

    @Test
    fun givenTheVpnRefused_whenShown_thenSaysFilteringCannotStartAndOffersItAgain() {
        capturePhone("OnboardingVpnRefused") { Onboarding(vpn(OnboardingStep.Install).copy(notifications = true, consentRefused = true)) }
    }

    @Test
    fun givenTheVpnStarted_whenTheListsDownload_thenShowsProgress() {
        capturePhone("OnboardingVpnStarting") {
            Onboarding(vpn(OnboardingStep.Install).copy(notifications = true, install = InstallState.Running(InstallStage.Lists, 0.6f)))
        }
    }

    @Test
    fun givenTheVpnRuns_whenShown_thenSaysWhatWasDone() {
        capturePhone("OnboardingVpnDone") { Onboarding(vpnDone()) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpnRuns_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "OnboardingVpnDone") { Onboarding(vpnDone()) }
    }

    @Test
    fun givenTheVpnRunsWithoutNotificationsAndLists_whenShown_thenSaysWhatIsLeft() {
        capturePhone("OnboardingVpnDonePartly") {
            Onboarding(
                vpn(OnboardingStep.Install).copy(
                    notifications = false,
                    install = InstallState.Done(active = true, apps = 5, listsFailed = true),
                ),
            )
        }
    }

    @Test
    fun givenNoEngineInThisBuild_whenTheVpnStarts_thenSaysSoAndOffersToTryAgain() {
        capturePhone("OnboardingVpnFailed") {
            Onboarding(vpn(OnboardingStep.Install).copy(notifications = true, install = InstallState.Failed(LoadFailure.EngineMissing)))
        }
    }

    @Test
    fun givenAllApps_whenSelectedInSetup_thenExplainsPersonalScope() {
        capturePhone("OnboardingScopeAll") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Apps, apps = apps, filteringScope = FilteringScope.All))
        }
    }

    @Test
    fun givenAppsStep_whenShown_thenChosenAppsIsTheDefault() {
        capturePhone("OnboardingApps") { Onboarding(OnboardingUiState(step = OnboardingStep.Apps, apps = apps)) }
    }

    @Test
    fun givenListsStep_whenShown_thenListsTheDefaults() {
        capturePhone("OnboardingLists") { Onboarding(OnboardingUiState(step = OnboardingStep.Lists)) }
    }

    @Test
    fun givenInstalling_whenShown_thenShowsProgress() {
        capturePhone("OnboardingInstalling") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Install, install = InstallState.Running(InstallStage.Lists, 0.6f)))
        }
    }

    @Test
    fun givenInstalledOnMagisk_whenShown_thenAsksForARestart() {
        capturePhone("OnboardingRestart") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Install, install = InstallState.Done(active = false, apps = 5)))
        }
    }

    @Test
    fun givenChecksUnderWay_whenShown_thenTheCurrentOneSpinsAndTheRestWait() {
        val checks = mapOf(DeviceCheckKind.Root to passed.getValue(DeviceCheckKind.Root))
        capturePhone("OnboardingChecking") { Onboarding(OnboardingUiState(step = OnboardingStep.Check, checks = checks)) }
    }

    @Test
    fun givenInstalledOnKernelSu_whenShown_thenSaysFilteringStarted() {
        capturePhone("OnboardingDone") {
            Onboarding(OnboardingUiState(step = OnboardingStep.Install, install = InstallState.Done(active = true, apps = 5)))
        }
    }

    @Test
    fun givenTheInstallerFailed_whenShown_thenSaysSoWithWhatItPrinted() {
        capturePhone("OnboardingInstallFailed") {
            Onboarding(
                OnboardingUiState(
                    step = OnboardingStep.Install,
                    install = InstallState.Failed(LoadFailure.Failed, detail = "! Unable to extract zip file"),
                ),
            )
        }
    }

    @Test
    @Config(qualifiers = "en-rUS-w412dp-h915dp")
    fun givenDeviceCheck_whenPredictiveBackCancelsThenCommits_thenItPreviewsWelcomeWithoutLeavingEarly() {
        val uiState = mutableStateOf(OnboardingUiState(step = OnboardingStep.Check, checks = passed))
        composeTestRule.setContent {
            Onboarding(uiState.value, onBack = {
                uiState.value.previousStep?.let { uiState.value = uiState.value.copy(step = it) }
            })
        }
        val dispatcher = composeTestRule.activity.onBackPressedDispatcher
        val welcome = composeTestRule.onNodeWithText("ADBlocker")
        val check = composeTestRule.onNodeWithText("Device check")
        check.assertIsDisplayed()
        welcome.assertDoesNotExist()
        fun swipeBack() {
            composeTestRule.runOnUiThread {
                dispatcher.dispatchOnBackStarted(BackEventCompat(411f, 420f, 0f, BackEventCompat.EDGE_RIGHT))
                dispatcher.dispatchOnBackProgressed(BackEventCompat(250f, 420f, 0.65f, BackEventCompat.EDGE_RIGHT))
            }
            composeTestRule.waitForIdle()
        }

        swipeBack()
        welcome.assertExists()
        composeTestRule.runOnUiThread { dispatcher.dispatchOnBackCancelled() }
        welcome.assertDoesNotExist()
        check.assertIsDisplayed()

        swipeBack()
        composeTestRule.runOnUiThread { dispatcher.onBackPressed() }
        welcome.assertIsDisplayed()
        check.assertDoesNotExist()
    }

    @Composable
    private fun Onboarding(state: OnboardingUiState, onBack: () -> Unit = {}) {
        OnboardingScreen(
            state = state,
            onNext = {},
            onBack = onBack,
            onRetryChecks = {},
            onAcknowledge = {},
            onTogglePick = {},
            onToggleList = {},
            onRetryInstall = {},
            onFinish = {},
        )
    }
}
