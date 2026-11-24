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
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.AdaptiveSheet
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.openSecuritySettings

/** What the settings of the VPN method do. */
class VpnActions(
    val onCheckRoot: () -> Unit = {},
    val onSwitchToRoot: () -> Unit = {},
    val onCancelRootSwitch: () -> Unit = {},
    val onOpenHttps: () -> Unit = {},
    val onTurnOffUrlFiltering: () -> Unit = {},
)

/**
 * The settings of filtering without root: the method, with root mode to
 * change to once root is granted; the VPN, whose start at boot and
 * lockdown the system holds; and HTTPS, which the user sets up and undoes.
 */
@Composable
internal fun VpnSettings(
    state: SettingsUiState,
    actions: VpnActions,
    onRemoveRootMode: () -> Unit,
    onUrlFiltering: (Boolean) -> Unit,
    onCosmetic: (Boolean) -> Unit,
    onShowPins: () -> Unit,
) {
    var confirmingRoot by rememberSaveable { mutableStateOf(false) }
    var removingCertificate by rememberSaveable { mutableStateOf(false) }
    var removingRootMode by rememberSaveable { mutableStateOf(false) }
    val context = LocalContext.current
    val service = state.service
    MethodRows(state, actions, onRemoveRootMode = { removingRootMode = true }) { confirmingRoot = true }
    VpnSystemRows(state, context)
    VpnHttpsRows(service, actions.onOpenHttps, onUrlFiltering, onCosmetic, onShowPins) { removingCertificate = true }

    if (confirmingRoot) {
        AlertDialog(
            onDismissRequest = { confirmingRoot = false },
            title = { Text(stringResource(R.string.feature_settings_switch_root_title)) },
            text = { Text(stringResource(R.string.feature_settings_switch_root_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmingRoot = false
                    actions.onSwitchToRoot()
                }) { Text(stringResource(R.string.feature_settings_switch_root)) }
            },
            dismissButton = { TextButton(onClick = { confirmingRoot = false }) { Text(stringResource(R.string.feature_settings_cancel)) } },
        )
    }
    if (removingRootMode) {
        RemoveRootModeDialog(
            onRemove = {
                removingRootMode = false
                onRemoveRootMode()
            },
        ) { removingRootMode = false }
    }
    if (removingCertificate) {
        RemoveCertificateSheet(
            onRemove = {
                removingCertificate = false
                actions.onTurnOffUrlFiltering()
                openSecuritySettings(context)
            },
        ) { removingCertificate = false }
    }
}

@Composable
private fun MethodRows(state: SettingsUiState, actions: VpnActions, onRemoveRootMode: () -> Unit, onConfirmRoot: () -> Unit) {
    val service = state.service
    val pending = state.userData.rootSwitchPending
    val cancelling = state.userData.rootSwitchCancelling
    // An installation of root mode, left by a cancelled switch for example, is removed here.
    // A module that needs a restart is said on the Root mode row itself.
    val restartFirst = service.module == ModuleState.NeedsRestart
    val installation = if (pending || restartFirst || service.rootAvailable != true) null else installationRow(service, onRemoveRootMode)
    val methodRows = 2 + (if (pending) 1 else 0) + (if (installation != null) 1 else 0)

    SectionHeader(stringResource(R.string.feature_settings_method))
    SegmentedGroup(methodRows) { index ->
        when (index) {
            0 -> SegmentedItem(
                index = 0,
                count = methodRows,
                headline = stringResource(R.string.feature_settings_method_vpn),
                supporting = { Text(stringResource(R.string.feature_settings_method_vpn_text)) },
                leading = { Icon(AdbIcons.VpnKey, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.Check, contentDescription = null) },
            )

            1 -> SegmentedItem(
                index = 1,
                count = methodRows,
                headline = stringResource(R.string.feature_settings_method_root),
                supporting = {
                    Text(
                        stringResource(
                            when {
                                service.checkingRoot -> R.string.feature_settings_method_root_checking

                                cancelling -> R.string.feature_settings_cancel_root_pending

                                service.switchingToRoot -> R.string.feature_settings_root_setting_up

                                // A switch would fail until the root manager has applied its change at a restart.
                                restartFirst -> if (service.stagedInManager) {
                                    R.string.feature_settings_method_root_restart_first
                                } else {
                                    R.string.feature_settings_root_marked_removal
                                }

                                pending -> when (service.module) {
                                    ModuleState.NoRoot -> R.string.feature_settings_method_root_missing
                                    ModuleState.Unavailable, null -> R.string.feature_settings_module_check_again
                                    ModuleState.Ready, ModuleState.NotInstalled, ModuleState.Mismatched, ModuleState.NeedsRestart -> R.string.feature_settings_switch_root_retry
                                }

                                service.rootAvailable == true -> R.string.feature_settings_method_root_available

                                else -> R.string.feature_settings_method_root_missing
                            },
                        ),
                    )
                },
                leading = { Icon(AdbIcons.Security, contentDescription = null, Modifier.size(20.dp)) },
                // Chosen after a confirmation once root is there; otherwise looked for again.
                onClick = when {
                    service.checkingRoot || service.busy || cancelling || restartFirst -> null

                    // Trying again sets up root mode where it has to.
                    pending -> when (service.module) {
                        ModuleState.Ready, ModuleState.NotInstalled, ModuleState.Mismatched, ModuleState.NeedsRestart -> actions.onSwitchToRoot
                        ModuleState.NoRoot, ModuleState.Unavailable, null -> actions.onCheckRoot
                    }

                    service.rootAvailable == true -> {
                        onConfirmRoot
                    }

                    else -> actions.onCheckRoot
                },
            )

            else -> if (installation != null) {
                RootRowItem(installation, index = 2, count = methodRows)
            } else {
                SegmentedItem(
                    index = 2,
                    count = methodRows,
                    headline = stringResource(if (cancelling) R.string.feature_settings_retry_root_cancel else R.string.feature_settings_cancel_root_switch),
                    leading = { Icon(AdbIcons.Close, contentDescription = null, Modifier.size(20.dp)) },
                    onClick = actions.onCancelRootSwitch.takeUnless { service.busy },
                )
            }
        }
    }
}

@Composable
private fun VpnSystemRows(state: SettingsUiState, context: Context) {
    val service = state.service
    SectionHeader(stringResource(R.string.feature_settings_vpn))
    SegmentedGroup(3) { index ->
        when (index) {
            0 -> SegmentedItem(
                index = 0,
                count = 3,
                headline = stringResource(R.string.feature_settings_always_on),
                supporting = {
                    Text(
                        stringResource(
                            R.string.feature_settings_state_detail,
                            stringResource(if (state.vpn.alwaysOn) R.string.feature_settings_state_on else R.string.feature_settings_state_off),
                            stringResource(R.string.feature_settings_always_on_text),
                        ),
                    )
                },
                leading = { Icon(AdbIcons.RestartAlt, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.OpenInNew, contentDescription = null) },
                onClick = { open(context, Intent(Settings.ACTION_VPN_SETTINGS)) },
            )

            // Set in the system only: said here, before the VPN ever stops.
            1 -> SegmentedItem(
                index = 1,
                count = 3,
                headline = stringResource(R.string.feature_settings_lockdown),
                supporting = {
                    Text(stringResource(if (state.vpn.lockdown) R.string.feature_settings_lockdown_on else R.string.feature_settings_lockdown_off))
                },
                leading = { Icon(AdbIcons.Warning, contentDescription = null, Modifier.size(20.dp)) },
            )

            else -> SegmentedItem(
                index = 2,
                count = 3,
                headline = stringResource(R.string.feature_settings_notifications),
                supporting = {
                    Text(stringResource(if (service.notificationsAllowed) R.string.feature_settings_notifications_on else R.string.feature_settings_notifications_off))
                },
                leading = { Icon(AdbIcons.Notifications, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.OpenInNew, contentDescription = null) },
                onClick = {
                    open(context, Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
                },
            )
        }
    }
}

@Composable
private fun VpnHttpsRows(
    service: ServiceInfo,
    onOpenHttps: () -> Unit,
    onUrlFiltering: (Boolean) -> Unit,
    onCosmetic: (Boolean) -> Unit,
    onShowPins: () -> Unit,
    onRemoveCertificate: () -> Unit,
) {
    // Decryption needs the certificate in the user store; the hosts apps
    // would not accept it for are shown once there are any.
    val installed = service.certificateInstalled
    val urlFiltering = service.status.valueOrNull?.urlFiltering
    val removable = installed
    val showPins = service.pinned is ReadState.Unavailable || service.pinned.valueOrNull?.isNotEmpty() == true
    val rows = 3 + (if (removable) 1 else 0) + (if (showPins) 1 else 0)
    SectionHeader(stringResource(R.string.feature_settings_https))
    SegmentedGroup(rows) { index ->
        when {
            index == 0 -> SegmentedItem(
                index = 0,
                count = rows,
                headline = stringResource(R.string.feature_settings_https_setup),
                supporting = {
                    Text(
                        stringResource(if (installed) R.string.feature_settings_https_installed else R.string.feature_settings_https_not_set_up),
                    )
                },
                leading = { Icon(AdbIcons.Lock, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
                onClick = onOpenHttps,
            )

            index == 1 -> UrlFilteringRow(
                index = 1,
                count = rows,
                checked = urlFiltering == true,
                // Also a choice without the certificate: it applies to cleartext HTTP.
                enabled = urlFiltering != null && !service.busy,
                text = when {
                    !installed -> R.string.feature_settings_url_filtering_needs_certificate
                    urlFiltering == null -> R.string.feature_settings_url_filtering_unknown
                    else -> R.string.feature_settings_url_filtering_text
                },
                onChange = onUrlFiltering,
            )

            index == 2 -> CosmeticRow(2, rows, service.status.valueOrNull?.cosmetic, service.busy, onCosmetic)

            index == 3 && removable -> SegmentedItem(
                index = 3,
                count = rows,
                headline = stringResource(R.string.feature_settings_remove_certificate),
                supporting = { Text(stringResource(R.string.feature_settings_remove_certificate_text)) },
                leading = { Icon(AdbIcons.Delete, contentDescription = null, Modifier.size(20.dp)) },
                onClick = onRemoveCertificate,
            )

            else -> PinnedHostsRow(index, rows, service.pinned, onShowPins)
        }
    }
}

/**
 * How the certificate is removed: Android lets only the user do it, in the
 * system settings. Decryption is turned off first, so that no app is left
 * set to be decrypted with a certificate that is gone.
 */
@Composable
internal fun RemoveCertificateSheet(onRemove: () -> Unit, onDismiss: () -> Unit) {
    AdaptiveSheet(onDismiss = onDismiss, title = stringResource(R.string.feature_settings_remove_certificate)) {
        Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.feature_settings_remove_certificate_explain), style = MaterialTheme.typography.bodyMedium)
            Text(stringResource(R.string.feature_settings_remove_certificate_path), style = MaterialTheme.typography.bodyMedium)
            Text(
                stringResource(R.string.feature_settings_remove_certificate_vary),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.feature_settings_cancel)) }
                Button(onClick = onRemove) { Text(stringResource(R.string.feature_settings_remove_certificate_open)) }
            }
        }
    }
}

/** A page of the system settings; the settings themselves where a device has no such page. */
private fun open(context: Context, intent: Intent) {
    try {
        context.startActivity(intent)
    } catch (_: ActivityNotFoundException) {
        context.startActivity(Intent(Settings.ACTION_SETTINGS))
    }
}
