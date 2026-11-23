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

import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.relativeTime
import li.mercury.adblocker.core.ui.rememberNow
import li.mercury.adblocker.core.ui.timeSince

/** Without root: the method and the VPN's state, how far HTTPS is set up, and the lists. */
@Composable
internal fun VpnStatusRows(state: OverviewUiState.Ready, onOpenFilters: () -> Unit, onOpenSettings: () -> Unit, onOpenHttps: () -> Unit) {
    val https = state.https
    SegmentedGroup(3) { index ->
        when (index) {
            0 -> SegmentedItem(
                index = 0,
                count = 3,
                headline = stringResource(R.string.feature_overview_method),
                supporting = {
                    Text(
                        stringResource(
                            when (state.vpn.state) {
                                VpnState.On -> R.string.feature_overview_method_vpn_on
                                VpnState.Starting -> R.string.feature_overview_method_vpn_connecting
                                VpnState.Off, VpnState.Revoked, VpnState.Failed -> R.string.feature_overview_method_vpn_off
                            },
                        ),
                    )
                },
                leading = { Icon(AdbIcons.VpnKey, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
                onClick = onOpenSettings,
            )

            1 -> SegmentedItem(
                index = 1,
                count = 3,
                headline = stringResource(R.string.feature_overview_https),
                supporting = {
                    // URL filtering is on by default: a removed certificate looks the same as one never installed.
                    Text(
                        when {
                            https == null || !https.certificateInstalled -> stringResource(R.string.feature_overview_https_not_set_up)
                            !https.urlFiltering -> stringResource(R.string.feature_overview_https_off)
                            https.decryptedApps == 0 -> stringResource(R.string.feature_overview_https_none)
                            else -> pluralStringResource(R.plurals.feature_overview_https_decrypted, https.decryptedApps, https.decryptedApps)
                        },
                    )
                },
                leading = { Icon(AdbIcons.Lock, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
                onClick = onOpenHttps,
            )

            else -> ListsRow(state, index = 2, count = 3, onOpenFilters)
        }
    }
}

@Composable
private fun ListsRow(state: OverviewUiState.Ready, index: Int, count: Int, onOpenFilters: () -> Unit) {
    val updated = state.listsUpdated?.let { relativeTime(it) }
    val enabled = state.enabledLists
    SegmentedItem(
        index = index,
        count = count,
        headline = stringResource(R.string.feature_overview_lists),
        supporting = {
            Text(
                if (updated == null) {
                    pluralStringResource(R.plurals.feature_overview_lists_never, enabled, enabled)
                } else {
                    pluralStringResource(R.plurals.feature_overview_lists_detail, enabled, enabled, updated)
                },
            )
        },
        leading = { Icon(AdbIcons.FilterAlt, contentDescription = null, Modifier.size(20.dp)) },
        trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
        onClick = onOpenFilters,
    )
}

@Composable
internal fun StatusRows(state: OverviewUiState.Ready, onOpenFilters: () -> Unit, onOpenSettings: () -> Unit) {
    val now = rememberNow()
    val started = state.status.startedAt?.let { timeSince(it, now) }
    SegmentedGroup(2) { index ->
        when (index) {
            0 -> ListsRow(state, index = 0, count = 2, onOpenFilters)

            else -> SegmentedItem(
                index = 1,
                count = 2,
                headline = stringResource(R.string.feature_overview_service),
                supporting = {
                    Text(
                        when {
                            !state.status.running -> stringResource(R.string.feature_overview_service_stopped)
                            started != null -> stringResource(R.string.feature_overview_service_running_since, started)
                            else -> stringResource(R.string.feature_overview_service_running)
                        },
                    )
                },
                leading = { Icon(AdbIcons.Dns, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
                onClick = onOpenSettings,
            )
        }
    }
}
