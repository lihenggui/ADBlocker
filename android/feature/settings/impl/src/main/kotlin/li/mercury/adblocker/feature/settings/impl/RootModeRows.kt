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

import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.data.repository.ModuleUpdate
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.relativeTime

/** What the rows about root mode do. */
class RootActions(
    val onRestart: () -> Unit = {},
    val onTurnOnAgain: () -> Unit = {},
    val onSetUp: () -> Unit = {},
    val onUpdate: () -> Unit = {},
    val onRemove: () -> Unit = {},
    val onCheckAgain: () -> Unit = {},
    val onSwitchToVpn: () -> Unit = {},
)

/** A row about root mode: what it says, and what a tap on it does, if anything. */
internal class RootRow(
    val headline: String,
    val supporting: String?,
    val icon: ImageVector,
    val onClick: (() -> Unit)? = null,
    val trailing: ImageVector? = null,
)

/**
 * Under root: the filtering service and its restart, setting root mode up
 * or bringing it up to date when it is not ready, where it is installed and
 * how it is removed, and the hosts apps would not accept the certificate for.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.ModuleService(state: SettingsUiState, actions: RootActions, onShowPins: () -> Unit) {
    val service = state.service
    var removing by rememberSaveable { mutableStateOf(false) }
    SectionHeader(stringResource(R.string.feature_settings_service))
    Reveal(service.busy, Modifier.align(Alignment.CenterHorizontally)) { LoadingIndicator() }
    val rows = listOfNotNull(
        statusRow(service),
        serviceActionRow(state, actions),
        setupRow(state, actions),
        installationRow(service) { removing = true },
    )
    val count = rows.size + 1
    SegmentedGroup(count) { index ->
        if (index < rows.size) RootRowItem(rows[index], index, count) else PinnedHostsRow(index, count, service.pinned, onShowPins)
    }
    if (removing) {
        RemoveRootModeDialog(
            onRemove = {
                removing = false
                actions.onRemove()
            },
        ) { removing = false }
    }
}

@Composable
internal fun RootRowItem(row: RootRow, index: Int, count: Int) {
    SegmentedItem(
        index = index,
        count = count,
        headline = row.headline,
        supporting = row.supporting?.let { { Text(it) } },
        leading = { Icon(row.icon, contentDescription = null, Modifier.size(20.dp)) },
        trailing = row.trailing?.let { { Icon(it, contentDescription = null) } },
        onClick = row.onClick,
    )
}

/** Nothing runs before root mode is set up, whatever waits in a root manager for a restart. */
private fun ServiceInfo.hasService() = module != ModuleState.NotInstalled

private fun ServiceInfo.turnedOff(): Boolean = status.valueOrNull?.let { !it.running && it.bootGuard != null } == true

@Composable
private fun statusRow(service: ServiceInfo): RootRow? {
    if (!service.hasService()) return null
    val status = service.status.valueOrNull
    val turnedOff = service.turnedOff()
    return RootRow(
        headline = stringResource(
            when {
                status == null -> R.string.feature_settings_status_unknown
                turnedOff -> R.string.feature_settings_status_turned_off
                !status.running -> R.string.feature_settings_status_stopped
                status.paused -> R.string.feature_settings_status_paused
                else -> R.string.feature_settings_status_running
            },
        ),
        supporting = when {
            turnedOff -> stringResource(R.string.feature_settings_turned_off_text)
            else -> status?.startedAt?.let { stringResource(R.string.feature_settings_started, relativeTime(it)) }
        },
        icon = AdbIcons.Dns,
    )
}

/** Restarting the service, or turning it on again after its startups failed; not while root mode is out of reach or changing. */
@Composable
private fun serviceActionRow(state: SettingsUiState, actions: RootActions): RootRow? {
    val service = state.service
    val reachable = service.module != ModuleState.NoRoot && service.module != ModuleState.Unavailable
    if (!service.hasService() || !reachable || state.update == ModuleUpdate.Updating || service.removal == RootTask.Running) return null
    if (service.turnedOff()) return RootRow(stringResource(R.string.feature_settings_turn_on_again), null, AdbIcons.RestartAlt, actions.onTurnOnAgain)
    return RootRow(
        stringResource(R.string.feature_settings_restart),
        stringResource(R.string.feature_settings_restart_text),
        AdbIcons.RestartAlt,
        actions.onRestart,
    )
}

/** Setting root mode up, bringing it up to date, or reaching it again: shown only while it is not ready. */
@Composable
private fun setupRow(state: SettingsUiState, actions: RootActions): RootRow? {
    val service = state.service
    val update = state.update
    return when {
        !service.hasService() -> RootRow(
            stringResource(R.string.feature_settings_module_install),
            stringResource(
                when (service.setup) {
                    RootTask.Running -> R.string.feature_settings_root_setting_up
                    RootTask.Failed -> R.string.feature_settings_root_setup_failed
                    RootTask.Idle -> R.string.feature_settings_root_not_set_up_text
                },
            ),
            AdbIcons.Download,
            actions.onSetUp.takeUnless { service.busy },
        )

        update == ModuleUpdate.Updating || service.module == ModuleState.Mismatched -> RootRow(
            stringResource(R.string.feature_settings_module_reinstall),
            stringResource(
                when (update) {
                    ModuleUpdate.Updating -> R.string.feature_settings_root_updating
                    is ModuleUpdate.Failed -> R.string.feature_settings_root_update_failed
                    ModuleUpdate.Idle -> R.string.feature_settings_root_update_text
                },
            ),
            AdbIcons.Download,
            actions.onUpdate.takeUnless { service.busy || update == ModuleUpdate.Updating },
        )

        service.module == ModuleState.NoRoot -> RootRow(
            stringResource(R.string.feature_settings_root_missing),
            stringResource(R.string.feature_settings_root_missing_text),
            AdbIcons.Warning,
            actions.onCheckAgain,
        )

        service.module == ModuleState.Unavailable -> RootRow(
            stringResource(R.string.feature_settings_module_unavailable),
            null,
            AdbIcons.Warning,
            actions.onCheckAgain,
        )

        else -> null
    }
}

/**
 * Where root mode is installed, which says how it is removed: here for an
 * installation the app made, in the root manager for a module flashed
 * there. A module that waits for a restart replaces the app's then.
 */
@Composable
internal fun installationRow(service: ServiceInfo, onRemove: () -> Unit): RootRow? {
    val manager = service.rootManager
    val inManager = if (manager != null) stringResource(R.string.feature_settings_root_flashed_in, manager) else stringResource(R.string.feature_settings_root_flashed)
    // Without the manager's name the headline stays short, and the line under it says where to go.
    val removeThere = stringResource(if (manager != null) R.string.feature_settings_root_flashed_text else R.string.feature_settings_root_flashed_manager_text)
    return when {
        service.removal == RootTask.Running -> RootRow(
            stringResource(R.string.feature_settings_root_remove),
            stringResource(R.string.feature_settings_root_removing),
            AdbIcons.Delete,
        )

        service.removal == RootTask.Failed && service.installedByApp == true -> RootRow(
            stringResource(R.string.feature_settings_root_remove),
            stringResource(R.string.feature_settings_root_remove_failed),
            AdbIcons.Delete,
            onRemove.takeUnless { service.busy },
        )

        // A module in use that the root manager changes at the restart: nothing here can, and the app does not restart the phone.
        service.module == ModuleState.NeedsRestart -> RootRow(
            inManager,
            stringResource(if (service.stagedInManager) R.string.feature_settings_root_restart_to_update else R.string.feature_settings_root_marked_removal),
            AdbIcons.Info,
        )

        // The manager has copied the new module's files but runs the old daemon until the restart.
        service.installedByApp == false && service.stagedInManager -> RootRow(
            inManager,
            stringResource(R.string.feature_settings_root_restart_to_update),
            AdbIcons.Info,
        )

        service.stagedInManager -> RootRow(inManager, stringResource(R.string.feature_settings_root_flashed_pending_text), AdbIcons.Info)

        service.installedByApp == true -> RootRow(
            stringResource(R.string.feature_settings_root_remove),
            stringResource(R.string.feature_settings_root_remove_text),
            AdbIcons.Delete,
            onRemove.takeUnless { service.busy },
        )

        service.installedByApp == false -> RootRow(inManager, removeThere, AdbIcons.Info)

        else -> null
    }
}

/** Removing root mode takes its certificate authority and its data with it: said, and confirmed, first. */
@Composable
internal fun RemoveRootModeDialog(onRemove: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.feature_settings_root_remove_title)) },
        text = { Text(stringResource(R.string.feature_settings_root_remove_explain)) },
        confirmButton = { TextButton(onClick = onRemove) { Text(stringResource(R.string.feature_settings_root_remove_confirm)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.feature_settings_cancel)) } },
    )
}

/**
 * Under root, with root mode not set up: the method, so that VPN mode can
 * be chosen instead. Nothing filters yet, so nothing is stopped.
 */
@Composable
internal fun RootMethodRows(busy: Boolean, onSwitchToVpn: () -> Unit) {
    SectionHeader(stringResource(R.string.feature_settings_method))
    SegmentedGroup(2) { index ->
        if (index == 0) {
            SegmentedItem(
                index = 0,
                count = 2,
                headline = stringResource(R.string.feature_settings_method_root),
                supporting = { Text(stringResource(R.string.feature_settings_method_root_not_set_up)) },
                leading = { Icon(AdbIcons.Security, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.Check, contentDescription = null) },
            )
        } else {
            SegmentedItem(
                index = 1,
                count = 2,
                headline = stringResource(R.string.feature_settings_method_vpn),
                supporting = { Text(stringResource(R.string.feature_settings_method_vpn_available)) },
                leading = { Icon(AdbIcons.VpnKey, contentDescription = null, Modifier.size(20.dp)) },
                onClick = onSwitchToVpn.takeUnless { busy },
            )
        }
    }
}
