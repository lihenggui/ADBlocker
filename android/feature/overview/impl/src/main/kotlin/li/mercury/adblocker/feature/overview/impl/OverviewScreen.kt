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

import android.app.Activity
import android.content.ActivityNotFoundException
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FailureView
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.pageMargin

@Composable
fun OverviewScreen(
    onOpenApp: (String) -> Unit,
    onOpenFilters: () -> Unit,
    onOpenSettings: () -> Unit,
    onOpenHttps: () -> Unit,
    viewModel: OverviewViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    // Another VPN app took the consent with the VPN: the system asks again.
    val consent = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) viewModel.reconnect() else viewModel.consentRefused()
    }
    val batteryExemption = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { viewModel.refresh() }
    // Back from elsewhere, at once rather than at the next tick.
    LifecycleStartEffect(viewModel) {
        viewModel.onStart()
        onStopOrDispose {}
    }
    OverviewScreen(
        state = state,
        onPause = viewModel::pause,
        onResume = viewModel::resume,
        onStartService = viewModel::startService,
        onTurnOnAgain = viewModel::turnOnAgain,
        onRetry = viewModel::refresh,
        onOpenApp = onOpenApp,
        onOpenFilters = onOpenFilters,
        onOpenSettings = onOpenSettings,
        modifier = modifier,
        onReconnect = { viewModel.consentIntent()?.let(consent::launch) ?: viewModel.reconnect() },
        onDismissNotice = viewModel::dismissNotice,
        onBatteryExemption = {
            // The screen can still disappear between resolving and launching it.
            try {
                viewModel.batteryExemptionIntent()?.let(batteryExemption::launch)
            } catch (_: ActivityNotFoundException) {
                viewModel.refresh()
            }
        },
        onSwitchToRoot = viewModel::switchToRoot,
        onOpenHttps = onOpenHttps,
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun OverviewScreen(
    state: OverviewUiState,
    onPause: (Int?) -> Unit,
    onResume: () -> Unit,
    onStartService: () -> Unit,
    onRetry: () -> Unit,
    onOpenApp: (String) -> Unit,
    onOpenFilters: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    onTurnOnAgain: () -> Unit = onStartService,
    onReconnect: () -> Unit = {},
    onDismissNotice: (OverviewNotice) -> Unit = {},
    onSwitchToRoot: () -> Unit = {},
    onBatteryExemption: () -> Unit = {},
    onOpenHttps: () -> Unit = {},
) {
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    Scaffold(
        modifier = modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_overview_title)) },
                actions = {
                    IconButton(onClick = onOpenSettings) {
                        Icon(AdbIcons.Settings, contentDescription = stringResource(R.string.feature_overview_settings))
                    }
                },
                scrollBehavior = scrollBehavior,
            )
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        // Keyed on the kind of state: each read of the status updates the page in place.
        FadeThrough(state, { it::class }, Modifier.padding(padding).fillMaxSize(), Alignment.Center, "overview") { shown ->
            when (shown) {
                OverviewUiState.Loading -> LoadingIndicator()

                is OverviewUiState.Failed -> FailureView(shown.failure, onRetry)

                is OverviewUiState.Ready -> OverviewContent(
                    state = shown,
                    actions = HeroActions(onPause, onResume, onStartService, onTurnOnAgain, onReconnect),
                    notices = NoticeActions(onDismissNotice, onSwitchToRoot, onBatteryExemption),
                    onOpenApp = onOpenApp,
                    onOpenFilters = onOpenFilters,
                    onOpenSettings = onOpenSettings,
                    onOpenHttps = onOpenHttps,
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun OverviewContent(
    state: OverviewUiState.Ready,
    actions: HeroActions,
    notices: NoticeActions,
    onOpenApp: (String) -> Unit,
    onOpenFilters: () -> Unit,
    onOpenSettings: () -> Unit,
    onOpenHttps: () -> Unit,
) {
    val compact = LocalCompactWindow.current
    val margin = pageMargin(compact)
    val first: @Composable () -> Unit = {
        StatusHero(state.status, state.filteredApps, actions, if (state.method == FilterMethod.Vpn) state.vpn else null)
        // Under the status it concerns; it folds away once dismissed.
        FadeThrough(state.notice, { it }, label = "notice") { notice ->
            if (notice != null) Notice(notice, notices) else Spacer(Modifier.fillMaxWidth())
        }
        Spacer(Modifier.height(12.dp))
        TodayCard(state.stats)
    }
    val second: @Composable () -> Unit = {
        // The first block of the day brings them in; the rows already there make room.
        RevealNotNull(state.topApps.ifEmpty { null }) { topApps ->
            Column {
                SectionHeader(stringResource(R.string.feature_overview_top_apps), trailing = stringResource(R.string.feature_overview_today))
                TopApps(topApps, onOpenApp)
            }
        }
        SectionHeader(stringResource(R.string.feature_overview_status))
        FadeThrough(state, { it.method }, label = "status") { state ->
            if (state.method == FilterMethod.Vpn) VpnStatusRows(state, onOpenFilters, onOpenSettings, onOpenHttps) else StatusRows(state, onOpenFilters, onOpenSettings)
        }
        Spacer(Modifier.height(24.dp))
    }
    val scroll = rememberScrollState()
    if (compact) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = margin)) {
            first()
            Spacer(Modifier.height(4.dp))
            second()
        }
    } else {
        // From medium up: two columns, the status beside what was blocked.
        Row(
            Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = margin),
            horizontalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            Column(Modifier.weight(1f).widthIn(max = 560.dp)) { first() }
            Column(Modifier.weight(1f).widthIn(max = 560.dp).padding(top = 8.dp)) { second() }
        }
    }
}
