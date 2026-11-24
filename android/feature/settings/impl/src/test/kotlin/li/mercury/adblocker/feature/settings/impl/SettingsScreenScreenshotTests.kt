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

package li.mercury.adblocker.feature.settings.impl

import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.test.assertHasClickAction
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import li.mercury.adblocker.core.data.repository.ModuleUpdate
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.UserData
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

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class SettingsScreenScreenshotTests {
    private val clock = DemoClockRule()
    private val composeTestRule = createAndroidComposeRule<ComponentActivity>()

    @get:Rule
    val rules: RuleChain = RuleChain.outerRule(clock).around(composeTestRule)

    private fun capturePhone(name: String, darkMode: Boolean = false, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice(if (darkMode) "phone_dark" else "phone", DefaultTestDevices.PHONE.spec, name, darkMode = darkMode, body = body)
    }

    private val state
        get() = SettingsUiState(
            service = ServiceInfo(
                status = ReadState.Available(DemoData.status),
                module = ModuleState.Ready,
                installedByApp = true,
                rootManager = "KernelSU",
                pinned = ReadState.Available(DemoData.pinned),
            ),
            appVersion = "0.1.0",
        )

    /** Root mode removed, or never set up: nothing filters, and VPN mode can be chosen instead. */
    private val notSetUp
        get() = state.copy(
            service = ServiceInfo(
                status = ReadState.Unavailable(LoadFailure.ModuleMissing),
                module = ModuleState.NotInstalled,
                rootManager = "Magisk",
                pinned = ReadState.Unavailable(LoadFailure.ModuleMissing),
                vpnAvailable = true,
            ),
        )

    /** A whole phone-wide page, down to the filtering service. */
    private fun captureTallPhone(name: String, body: @Composable () -> Unit) {
        composeTestRule.captureForDevice("phone_tall", TALL_PHONE, name, body = body)
    }

    /** Without root, before the HTTPS setup: URL filtering on by default, no certificate so nothing decrypted, and root not granted. */
    private val vpn
        get() = SettingsUiState(
            userData = UserData(filterMethod = FilterMethod.Vpn),
            service = ServiceInfo(
                status = ReadState.Available(DemoData.status),
                rootAvailable = false,
            ),
            appVersion = "0.1.0",
            vpn = VpnStatus(state = VpnState.On),
        )

    /** Without root, after the HTTPS setup, with the system's VPN settings on and root granted since. */
    private val vpnSetUp
        get() = vpn.copy(
            service = vpn.service.copy(status = ReadState.Available(DemoData.status), certificateInstalled = true, rootAvailable = true, notificationsAllowed = false),
            vpn = VpnStatus(state = VpnState.On, alwaysOn = true, lockdown = true),
        )

    @Test
    fun givenTheVpn_whenShown_thenTheMethodTheVpnAndHttpsComeFirst() {
        capturePhone("SettingsVpn") { Settings(vpn) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpn_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "SettingsVpn") { Settings(vpn) }
    }

    @Test
    fun givenTheVpnOnATablet_whenShown_thenMatchesGolden() {
        composeTestRule.captureForDevice("tablet", DefaultTestDevices.TABLET.spec, "SettingsVpn") { Settings(vpn) }
    }

    @Test
    fun givenTheVpnWithHttpsSetUpAndRootGranted_whenShown_thenOffersRootAndTheCertificatesRemoval() {
        capturePhone("SettingsVpnSetUp") { Settings(vpnSetUp) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheVpnWithHttpsSetUp_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "SettingsVpnSetUp") { Settings(vpnSetUp) }
    }

    @Test
    fun givenTheCertificateInstalled_whenItsRemovalIsAsked_thenSaysHowAndWhereFirst() {
        capturePhone("SettingsRemoveCertificateSheet") { RemoveCertificateSheet(onRemove = {}, onDismiss = {}) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenTheCertificateInstalled_whenItsRemovalIsAskedInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "SettingsRemoveCertificateSheet") {
            RemoveCertificateSheet(onRemove = {}, onDismiss = {})
        }
    }

    @Test
    fun givenRunning_whenShownOnEachDevice_thenMatchesGolden() {
        composeTestRule.captureMultiDevice("Settings") { Settings(state) }
    }

    @Test
    fun givenHttp3Diagnostics_whenSettingsAreShown_thenServiceAndDataAreAdjacent() {
        composeTestRule.captureForDevice(
            "tablet_portrait",
            "spec:shape=Normal,width=800,height=1500,unit=dp,dpi=240",
            "Settings",
        ) { Settings(state) }
    }

    @Test
    fun givenRunning_whenShownInDarkTheme_thenMatchesGolden() {
        capturePhone("Settings", darkMode = true) { Settings(state) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRunning_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "Settings") { Settings(state) }
    }

    @Test
    fun givenDecryptionOff_whenShown_thenTheSwitchSaysSo() {
        val status = DemoData.status.copy(urlFiltering = false)
        capturePhone("SettingsDecryptionOff") { Settings(state.copy(service = state.service.copy(status = ReadState.Available(status)))) }
    }

    @Test
    fun givenDifferentDaemon_whenShown_thenOffersTheBundledModule() {
        composeTestRule.captureForDevice(
            "tablet_portrait",
            "spec:shape=Normal,width=800,height=1500,unit=dp,dpi=240",
            "SettingsMismatched",
        ) { Settings(state.copy(service = state.service.copy(module = ModuleState.Mismatched))) }
        composeTestRule.onNodeWithText(composeTestRule.activity.getString(R.string.feature_settings_module_reinstall)).assertHasClickAction()
    }

    @Test
    fun givenHidingOff_whenTurnedOn_thenExplainsWhatItChangesFirst() {
        capturePhone("SettingsCosmeticSheet") { CosmeticSheet(onTurnOn = {}, onOpenSecurity = {}, onDismiss = {}) }
    }

    @Test
    fun givenPinnedHosts_whenOpened_thenListsThemByApp() {
        capturePhone("SettingsPinnedSheet") { PinnedSheet(DemoData.pinned, emptyMap(), onForget = { _, _ -> }, onForgetAll = {}, onDismiss = {}) }
    }

    @Test
    fun givenRootModeNotSetUp_whenShown_thenOffersToSetItUpOrToChooseVpnMode() {
        captureTallPhone("SettingsNotSetUp") { Settings(notSetUp) }
        composeTestRule.onNodeWithText(composeTestRule.activity.getString(R.string.feature_settings_module_install)).assertHasClickAction()
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRootModeNotSetUp_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_tall_zh", TALL_PHONE, "SettingsNotSetUp") { Settings(notSetUp) }
    }

    @Test
    fun givenSettingUpFailed_whenShown_thenTheRowSaysSoAndOffersItAgain() {
        captureTallPhone("SettingsSetupFailed") { Settings(notSetUp.copy(service = notSetUp.service.copy(setup = RootTask.Failed))) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRootModeSetUpByTheApp_whenShownInChinese_thenOffersToRemoveIt() {
        composeTestRule.captureForDevice("phone_tall_zh", TALL_PHONE, "SettingsRootMode") { Settings(state) }
    }

    @Test
    fun givenRootModeSetUpByTheApp_whenShown_thenOffersToRemoveIt() {
        captureTallPhone("SettingsRootMode") { Settings(state) }
        composeTestRule.onNodeWithText(composeTestRule.activity.getString(R.string.feature_settings_root_remove)).assertHasClickAction()
    }

    @Test
    fun givenRemovingRootMode_whenConfirming_thenSaysWhatIsDeletedFirst() {
        capturePhone("SettingsRemoveRootModeDialog") { RemoveRootModeDialog(onRemove = {}, onDismiss = {}) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenRemovingRootMode_whenConfirmingInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_zh", DefaultTestDevices.PHONE.spec, "SettingsRemoveRootModeDialog") {
            RemoveRootModeDialog(onRemove = {}, onDismiss = {})
        }
    }

    @Test
    fun givenAModuleFlashedInAManager_whenShown_thenSaysToRemoveItThere() {
        captureTallPhone("SettingsFlashed") {
            Settings(state.copy(service = state.service.copy(installedByApp = false, rootManager = "Magisk")))
        }
    }

    @Test
    fun givenAModuleFlashedInAManagerNotFound_whenShown_thenSaysToRemoveItInTheRootManager() {
        captureTallPhone("SettingsFlashedUnknownManager") {
            Settings(state.copy(service = state.service.copy(installedByApp = false, rootManager = null)))
        }
    }

    @Test
    fun givenAModuleFlashedInTheManagerWithTheLongestName_whenShown_thenTheHeadlineFits() {
        captureTallPhone("SettingsFlashedKernelSu") {
            Settings(state.copy(service = state.service.copy(installedByApp = false, rootManager = "KernelSU")))
        }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAModuleFlashedInTheManagerWithTheLongestName_whenShownInChinese_thenTheHeadlineFits() {
        composeTestRule.captureForDevice("phone_tall_zh", TALL_PHONE, "SettingsFlashedKernelSu") {
            Settings(state.copy(service = state.service.copy(installedByApp = false, rootManager = "KernelSU")))
        }
    }

    private val heldForUpdate
        get() = state.copy(service = state.service.copy(module = ModuleState.NeedsRestart, installedByApp = null, stagedInManager = true, rootManager = "Magisk"))

    private val heldForRemoval
        get() = heldForUpdate.copy(service = heldForUpdate.service.copy(stagedInManager = false))

    @Test
    fun givenAModuleWithAnUpdateWaitingInItsManager_whenShown_thenSaysARestartFinishesIt() {
        captureTallPhone("SettingsRestartToUpdate") { Settings(heldForUpdate) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAModuleWithAnUpdateWaitingInItsManager_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_tall_zh", TALL_PHONE, "SettingsRestartToUpdate") { Settings(heldForUpdate) }
    }

    @Test
    fun givenAModuleMarkedForRemovalInItsManager_whenShown_thenSaysARestartRemovesIt() {
        captureTallPhone("SettingsMarkedForRemoval") { Settings(heldForRemoval) }
    }

    @Test
    @Config(qualifiers = "zh-rCN")
    fun givenAModuleMarkedForRemovalInItsManager_whenShownInChinese_thenMatchesGolden() {
        composeTestRule.captureForDevice("phone_tall_zh", TALL_PHONE, "SettingsMarkedForRemoval") { Settings(heldForRemoval) }
    }

    @Test
    fun givenAModuleWithAnUpdateWaiting_whenTheVpnIsInUse_thenRootModeIsNotOfferedUntilARestart() {
        val held = vpnSetUp.copy(service = vpnSetUp.service.copy(module = ModuleState.NeedsRestart, stagedInManager = true))
        capturePhone("SettingsVpnRestartFirst") { Settings(held) }
    }

    @Test
    fun givenAModuleFlashedAndWaitingForARestart_whenTheAppsCopyRuns_thenSaysWhenTheModuleTakesEffect() {
        captureTallPhone("SettingsFlashedPending") {
            Settings(state.copy(service = state.service.copy(stagedInManager = true, rootManager = "APatch")))
        }
    }

    @Test
    fun givenTwoStartupsDidNotFinish_whenShown_thenOffersToTurnRootModeOnAgain() {
        val status = DemoData.status.copy(running = false, startedAt = null, bootGuard = DemoData.status.startedAt)
        captureTallPhone("SettingsTurnedOff") { Settings(state.copy(service = state.service.copy(status = ReadState.Available(status)))) }
    }

    @Test
    fun givenAnUpdateFailed_whenShown_thenTheUpdateRowOffersItAgain() {
        captureTallPhone("SettingsUpdateFailed") {
            Settings(state.copy(service = state.service.copy(module = ModuleState.Mismatched), update = ModuleUpdate.Failed(LoadFailure.Failed)))
        }
    }

    @Test
    fun givenASwitchToRootThatDidNotFinish_whenShown_thenOffersToTryAgain() {
        val pending = vpnSetUp.copy(
            userData = vpnSetUp.userData.copy(rootSwitchPending = true),
            service = vpnSetUp.service.copy(module = ModuleState.NotInstalled),
        )
        capturePhone("SettingsVpnSwitchUnfinished") { Settings(pending) }
    }

    @Test
    fun givenRootModeLeftInstalledWhileTheVpnIsInUse_whenShown_thenOffersToRemoveIt() {
        val left = vpnSetUp.copy(service = vpnSetUp.service.copy(module = ModuleState.Ready, installedByApp = true))
        capturePhone("SettingsVpnRootModeLeft") { Settings(left) }
    }

    @Composable
    private fun Settings(state: SettingsUiState) {
        SettingsScreen(
            state = state,
            onBack = {},
            onThemeMode = {},
            onDynamicColor = {},
            onUpdateInterval = {},
            onWifiOnly = {},
            onShowSystemApps = {},
            onUrlFiltering = {},
            onForgetAllPins = {},
            onExportLogs = {},
            rootActions = RootActions(onSetUp = {}, onRemove = {}),
        )
    }

    private companion object {
        const val TALL_PHONE = "spec:shape=Normal,width=412,height=2000,unit=dp,dpi=480"
    }
}
