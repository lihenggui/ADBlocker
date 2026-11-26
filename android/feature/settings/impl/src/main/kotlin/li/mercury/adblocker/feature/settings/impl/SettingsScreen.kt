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

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MediumFlexibleTopAppBar
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.pageMargin

/** Where the source code lives, shown under About. */
private const val SOURCE_URL = "https://github.com/lihenggui/ADBlocker"

private const val SECURITY_URL = "$SOURCE_URL/blob/main/SECURITY.md#hiding-page-elements"

@Composable
fun SettingsScreen(onBack: () -> Unit, onOpenHttps: () -> Unit, viewModel: SettingsViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    val logs = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri -> uri?.let(viewModel::exportLogs) }
    SettingsScreen(
        state = state,
        onBack = onBack,
        onThemeMode = viewModel::setThemeMode,
        onDynamicColor = viewModel::setUseDynamicColor,
        onUpdateInterval = viewModel::setUpdateInterval,
        onWifiOnly = viewModel::setUpdateOnWifiOnly,
        onShowSystemApps = viewModel::setShowSystemApps,
        onFilteringScope = viewModel::setFilteringScope,
        onUrlFiltering = viewModel::setUrlFiltering,
        onForgetAllPins = viewModel::forgetAllPinned,
        onExportLogs = { logs.launch("adblocker-log.txt") },
        modifier = modifier,
        onCosmetic = viewModel::setCosmetic,
        onForgetPin = viewModel::forgetPinned,
        rootActions = RootActions(
            onRestart = viewModel::restartService,
            onTurnOnAgain = viewModel::turnOnAgain,
            onSetUp = viewModel::setUpRootMode,
            onUpdate = viewModel::updateRootMode,
            onRemove = viewModel::removeRootMode,
            onCheckAgain = viewModel::refresh,
            onSwitchToVpn = viewModel::switchToVpn,
        ),
        vpnActions = VpnActions(
            onCheckRoot = { viewModel.checkRoot() },
            onSwitchToRoot = viewModel::switchToRoot,
            onCancelRootSwitch = viewModel::cancelRootSwitch,
            onOpenHttps = onOpenHttps,
            onTurnOffUrlFiltering = viewModel::turnOffUrlFiltering,
        ),
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun SettingsScreen(
    state: SettingsUiState,
    onBack: () -> Unit,
    onThemeMode: (ThemeMode) -> Unit,
    onDynamicColor: (Boolean) -> Unit,
    onUpdateInterval: (UpdateInterval) -> Unit,
    onWifiOnly: (Boolean) -> Unit,
    onShowSystemApps: (Boolean) -> Unit,
    onUrlFiltering: (Boolean) -> Unit,
    onForgetAllPins: () -> Unit,
    onExportLogs: () -> Unit,
    modifier: Modifier = Modifier,
    onFilteringScope: (FilteringScope) -> Unit = {},
    onCosmetic: (Boolean) -> Unit = {},
    onForgetPin: (packageName: String, host: String?) -> Unit = { _, _ -> },
    rootActions: RootActions = RootActions(),
    vpnActions: VpnActions = VpnActions(),
) {
    val scrollBehavior = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    var choosingInterval by rememberSaveable { mutableStateOf(false) }
    var showingPinned by rememberSaveable { mutableStateOf(false) }
    val context = LocalContext.current
    val openCosmeticSecurity = { context.openLink(SECURITY_URL) }
    val data = state.userData
    val service = state.service
    Scaffold(
        modifier = modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = { SettingsBar(scrollBehavior, onBack) },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        Column(
            Modifier.padding(padding).fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth)
                .verticalScroll(rememberScrollState()).padding(horizontal = pageMargin(LocalCompactWindow.current)),
        ) {
            // Without root, how filtering is done comes first: the method, the
            // VPN it takes, and HTTPS, which the user sets up.
            val usesVpn = data.capabilities.usesVpn
            if (usesVpn) {
                VpnSettings(
                    state = state,
                    actions = vpnActions,
                    onRemoveRootMode = rootActions.onRemove,
                    onUrlFiltering = onUrlFiltering,
                    onCosmetic = onCosmetic,
                    onOpenCosmeticSecurity = openCosmeticSecurity,
                    onShowPins = { showingPinned = true },
                )
            } else if (service.module == ModuleState.NotInstalled && service.vpnAvailable) {
                // Nothing filters until root mode is set up: VPN mode can be chosen instead.
                RootMethodRows(service.busy, rootActions.onSwitchToVpn)
            }

            AppearanceRows(data.themeMode, data.useDynamicColor, onThemeMode, onDynamicColor)
            UpdateRows(
                interval = data.updateInterval,
                wifiOnly = data.updateOnWifiOnly,
                choosingInterval = choosingInterval,
                onChooseInterval = { choosingInterval = it },
                onUpdateInterval = onUpdateInterval,
                onWifiOnly = onWifiOnly,
            )

            AppListSettingsRows(service, data.showSystemApps, onFilteringScope, onShowSystemApps)

            if (!usesVpn) {
                RootHttpsRows(service, onUrlFiltering, onCosmetic, openCosmeticSecurity)
                ModuleService(state, rootActions) { showingPinned = true }
            }

            DataAndAboutRows(state.appVersion, onExportLogs, onOpenSource = { context.openLink(SOURCE_URL) })
            Spacer(Modifier.height(32.dp))
        }
    }
    val pinned = service.pinned.valueOrNull
    if (showingPinned && !pinned.isNullOrEmpty()) {
        PinnedSheet(pinned, service.pinnedLabels, onForget = onForgetPin, onForgetAll = onForgetAllPins) { showingPinned = false }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SettingsBar(scrollBehavior: TopAppBarScrollBehavior, onBack: () -> Unit) {
    MediumFlexibleTopAppBar(
        title = { Text(stringResource(R.string.feature_settings_title)) },
        navigationIcon = {
            IconButton(onClick = onBack) { Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_settings_back)) }
        },
        scrollBehavior = scrollBehavior,
    )
}

private fun Context.openLink(url: String) {
    try {
        startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
    } catch (_: ActivityNotFoundException) {
        // Devices without a browser stay on the settings page.
    }
}
