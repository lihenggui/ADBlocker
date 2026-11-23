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
import li.mercury.adblocker.core.model.FilterMethod
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
                moduleVersion = "v0.1.0",
                rootManager = "KernelSU",
                pinned = ReadState.Available(DemoData.pinned),
            ),
            appVersion = "0.1.0",
        )

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
            onRestart = {},
            onInstallModule = {},
            onForgetAllPins = {},
            onExportLogs = {},
        )
    }
}
