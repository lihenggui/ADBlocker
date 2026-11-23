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

import android.os.Build
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuAnchorPosition
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SelectableDropdownMenuItem
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.ui.ConnectedChoice
import li.mercury.adblocker.core.ui.ExpandChevron
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FilteringScopeChoice
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.SegmentedToggleItem
import li.mercury.adblocker.core.ui.relativeTime
import li.mercury.adblocker.core.ui.segmentedColors
import li.mercury.adblocker.core.ui.R as CoreUiR

/** Under root: the daemon, its restart, the module, and the hosts apps would not accept the certificate for. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.ModuleService(state: SettingsUiState, onRestart: () -> Unit, onInstallModule: () -> Unit, onShowPins: () -> Unit) {
    val service = state.service
    val status = service.status.valueOrNull
    SectionHeader(stringResource(R.string.feature_settings_service))
    Reveal(service.busy, Modifier.align(Alignment.CenterHorizontally)) { LoadingIndicator() }
    val rows = 4
    SegmentedGroup(rows) { index ->
        when (index) {
            0 -> SegmentedItem(
                index = 0,
                count = rows,
                headline = stringResource(
                    when {
                        status == null -> R.string.feature_settings_status_unknown
                        !status.running -> R.string.feature_settings_status_stopped
                        status.paused -> R.string.feature_settings_status_paused
                        else -> R.string.feature_settings_status_running
                    },
                ),
                supporting = {
                    Text(
                        listOfNotNull(
                            status?.startedAt?.let { stringResource(R.string.feature_settings_started, relativeTime(it)) },
                            service.moduleVersion?.let { stringResource(R.string.feature_settings_module_version, it) },
                            service.rootManager,
                        ).joinToString(" · "),
                    )
                },
                leading = { Icon(AdbIcons.Dns, contentDescription = null, Modifier.size(20.dp)) },
            )

            1 -> SegmentedItem(
                index = 1,
                count = rows,
                headline = stringResource(R.string.feature_settings_restart),
                supporting = { Text(stringResource(R.string.feature_settings_restart_text)) },
                leading = { Icon(AdbIcons.RestartAlt, contentDescription = null, Modifier.size(20.dp)) },
                onClick = onRestart,
            )

            2 -> SegmentedItem(
                index = 2,
                count = rows,
                headline = stringResource(
                    when (service.module) {
                        ModuleState.NotInstalled -> R.string.feature_settings_module_install
                        ModuleState.Mismatched -> R.string.feature_settings_module_reinstall
                        ModuleState.NeedsRestart -> R.string.feature_settings_module_needs_restart
                        ModuleState.Ready -> R.string.feature_settings_module_current
                        ModuleState.NoRoot -> R.string.feature_settings_root_missing
                        ModuleState.Unavailable, null -> R.string.feature_settings_module_unavailable
                    },
                ),
                supporting = { Text(stringResource(R.string.feature_settings_module_text)) },
                leading = { Icon(AdbIcons.Download, contentDescription = null, Modifier.size(20.dp)) },
                onClick = if (service.module == ModuleState.NotInstalled || service.module == ModuleState.Mismatched) onInstallModule else null,
            )

            else -> PinnedHostsRow(index, rows, service.pinned, onShowPins)
        }
    }
}

@Composable
internal fun PinnedHostsRow(index: Int, count: Int, pins: ReadState<List<PinnedHost>>, onShowPins: () -> Unit) {
    val available = pins.valueOrNull?.isNotEmpty() == true
    SegmentedItem(
        index = index,
        count = count,
        headline = stringResource(R.string.feature_settings_pinned),
        supporting = {
            val detail = when (pins) {
                ReadState.Pending -> stringResource(R.string.feature_settings_pinned_checking)

                is ReadState.Unavailable -> stringResource(R.string.feature_settings_pinned_unavailable)

                is ReadState.Available -> {
                    val domains = pins.value.size
                    if (domains == 0) {
                        stringResource(R.string.feature_settings_pinned_none)
                    } else {
                        val apps = pins.value.map(PinnedHost::appKey).distinct().size
                        pluralStringResource(R.plurals.feature_settings_pinned_apps, apps, apps) + " · " +
                            pluralStringResource(R.plurals.feature_settings_pinned_domains, domains, domains)
                    }
                }
            }
            Text(detail)
        },
        leading = { Icon(AdbIcons.Key, contentDescription = null, Modifier.size(20.dp)) },
        trailing = if (available) {
            { Icon(AdbIcons.ChevronRight, contentDescription = null) }
        } else {
            null
        },
        onClick = onShowPins.takeIf { available },
    )
}

/** Whether anything is decrypted; [text] says what it does, or why it cannot be changed. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun UrlFilteringRow(index: Int, count: Int, checked: Boolean, enabled: Boolean, text: Int, onChange: (Boolean) -> Unit) {
    SegmentedToggleItem(
        index = index,
        count = count,
        headline = stringResource(R.string.feature_settings_url_filtering),
        checked = checked,
        onCheckedChange = onChange,
        enabled = enabled,
        supporting = { Text(stringResource(text)) },
        trailing = { SettingSwitch(checked, enabled) },
    )
}

/** Hiding page elements: known once a daemon that knows the command says, as [cosmetic]. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun CosmeticRow(index: Int, count: Int, cosmetic: Boolean?, busy: Boolean, onChange: (Boolean) -> Unit) {
    val enabled = cosmetic != null && !busy
    SegmentedToggleItem(
        index = index,
        count = count,
        headline = stringResource(R.string.feature_settings_cosmetic),
        checked = cosmetic == true,
        onCheckedChange = onChange,
        enabled = enabled,
        supporting = {
            Text(stringResource(if (cosmetic == null) R.string.feature_settings_url_filtering_unknown else R.string.feature_settings_cosmetic_text))
        },
        trailing = { SettingSwitch(cosmetic == true, enabled) },
    )
}

/** A setting's switch: the row handles the click; it shows the state, with a check while on. */
@Composable
internal fun SettingSwitch(checked: Boolean, enabled: Boolean = true) {
    Switch(
        checked = checked,
        onCheckedChange = null,
        enabled = enabled,
        thumbContent = if (checked) {
            { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
        } else {
            null
        },
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun Toggle(index: Int, count: Int, title: Int, text: Int, checked: Boolean, onChange: (Boolean) -> Unit) {
    SegmentedToggleItem(
        index = index,
        count = count,
        headline = stringResource(title),
        checked = checked,
        onCheckedChange = onChange,
        supporting = { Text(stringResource(text)) },
        trailing = {
            Switch(
                checked = checked,
                onCheckedChange = null,
                thumbContent = if (checked) {
                    { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
                } else {
                    null
                },
            )
        },
    )
}

private fun intervalLabel(interval: UpdateInterval) = when (interval) {
    UpdateInterval.Daily -> R.string.feature_settings_interval_daily
    UpdateInterval.EveryThreeDays -> R.string.feature_settings_interval_three_days
    UpdateInterval.Weekly -> R.string.feature_settings_interval_weekly
    UpdateInterval.Never -> R.string.feature_settings_interval_never
}

/** The update intervals, as a menu hanging from the row; the chosen one is marked. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun IntervalMenu(expanded: Boolean, selected: UpdateInterval, onSelect: (UpdateInterval) -> Unit, onDismiss: () -> Unit) {
    DropdownMenuPopup(
        expanded = expanded,
        onDismissRequest = onDismiss,
        popupPositionProvider = MenuDefaults.rememberDropdownMenuPopupPositionProvider(MenuAnchorPosition.Below, DpOffset(0.dp, 4.dp)),
    ) {
        DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
            UpdateInterval.entries.forEachIndexed { index, interval ->
                SelectableDropdownMenuItem(
                    selected = interval == selected,
                    onClick = { onSelect(interval) },
                    text = { Text(stringResource(intervalLabel(interval))) },
                    shapes = MenuDefaults.itemShape(index, UpdateInterval.entries.size),
                    selectedLeadingIcon = { Icon(AdbIcons.Check, contentDescription = null) },
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun AppearanceRows(themeMode: ThemeMode, dynamicColor: Boolean, onThemeMode: (ThemeMode) -> Unit, onDynamicColor: (Boolean) -> Unit) {
    SectionHeader(stringResource(R.string.feature_settings_appearance))
    // Wallpaper colors came with Android 12.
    val appearanceRows = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) 2 else 1
    SegmentedGroup(appearanceRows) { index ->
        if (index == 0) {
            SegmentedListItem(
                shapes = ListItemDefaults.segmentedShapes(0, appearanceRows),
                colors = segmentedColors(),
                supportingContent = {
                    ConnectedChoice(
                        options = ThemeMode.entries,
                        selected = themeMode,
                        onSelect = onThemeMode,
                        label = {
                            stringResource(
                                when (it) {
                                    ThemeMode.System -> R.string.feature_settings_theme_system
                                    ThemeMode.Light -> R.string.feature_settings_theme_light
                                    ThemeMode.Dark -> R.string.feature_settings_theme_dark
                                },
                            )
                        },
                        modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
                        inListItem = true,
                    )
                },
            ) { Text(stringResource(R.string.feature_settings_theme)) }
        } else {
            Toggle(1, 2, R.string.feature_settings_dynamic_color, R.string.feature_settings_dynamic_color_text, dynamicColor, onDynamicColor)
        }
    }
}

@Composable
internal fun UpdateRows(
    interval: UpdateInterval,
    wifiOnly: Boolean,
    choosingInterval: Boolean,
    onChooseInterval: (Boolean) -> Unit,
    onUpdateInterval: (UpdateInterval) -> Unit,
    onWifiOnly: (Boolean) -> Unit,
) {
    SectionHeader(stringResource(R.string.feature_settings_updates))
    SegmentedGroup(2) { index ->
        if (index == 0) {
            // A menu of the intervals, hanging from the row's end.
            Box {
                SegmentedItem(
                    index = 0,
                    count = 2,
                    headline = stringResource(R.string.feature_settings_update_interval),
                    supporting = { Text(stringResource(intervalLabel(interval))) },
                    leading = { Icon(AdbIcons.Schedule, contentDescription = null, Modifier.size(20.dp)) },
                    trailing = { ExpandChevron(choosingInterval) },
                    onClick = { onChooseInterval(true) },
                )
                Box(Modifier.align(Alignment.BottomEnd)) {
                    IntervalMenu(choosingInterval, interval, onSelect = {
                        onChooseInterval(false)
                        onUpdateInterval(it)
                    }) { onChooseInterval(false) }
                }
            }
        } else {
            Toggle(1, 2, R.string.feature_settings_wifi_only, R.string.feature_settings_wifi_only_text, wifiOnly, onWifiOnly)
        }
    }
}

@Composable
internal fun AppListSettingsRows(
    service: ServiceInfo,
    showSystemApps: Boolean,
    onFilteringScope: (FilteringScope) -> Unit,
    onShowSystemApps: (Boolean) -> Unit,
) {
    // The scope is known once the daemon answers: the choice fades in where the wait was said.
    FadeThrough(service, { it.status::class }, label = "scope") { service ->
        val status = service.status.valueOrNull
        if (status != null) {
            FilteringScopeChoice(status.filteringScope, onFilteringScope, enabled = !service.busy)
        } else {
            Column {
                SectionHeader(stringResource(CoreUiR.string.core_ui_filtering_scope))
                Text(
                    stringResource(
                        if (service.status is ReadState.Unavailable) R.string.feature_settings_scope_unavailable else R.string.feature_settings_scope_checking,
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
    }
    SectionHeader(stringResource(R.string.feature_settings_app_list))
    Toggle(0, 1, R.string.feature_settings_system_apps, R.string.feature_settings_system_apps_text, showSystemApps, onShowSystemApps)
}

@Composable
internal fun RootHttpsRows(service: ServiceInfo, onUrlFiltering: (Boolean) -> Unit, onCosmetic: (Boolean) -> Unit) {
    // Whether anything is decrypted: a running daemon of a module
    // that knows the command says.
    val urlFiltering = service.status.valueOrNull?.urlFiltering
    SectionHeader(stringResource(R.string.feature_settings_https))
    val cosmetic = service.status.valueOrNull?.cosmetic
    SegmentedGroup(2) { row ->
        if (row == 1) {
            // Turning it on changes pages: it is explained first. Off needs no word.
            CosmeticRow(1, 2, cosmetic, service.busy, onCosmetic)
        } else {
            UrlFilteringRow(
                index = 0,
                count = 2,
                checked = urlFiltering == true,
                enabled = urlFiltering != null && !service.busy,
                text = if (urlFiltering == null) R.string.feature_settings_url_filtering_unknown else R.string.feature_settings_url_filtering_text,
                onChange = onUrlFiltering,
            )
        }
    }
}

@Composable
internal fun DataAndAboutRows(appVersion: String, onExportLogs: () -> Unit, onOpenSource: () -> Unit) {
    SectionHeader(stringResource(R.string.feature_settings_data))
    SegmentedGroup(1) {
        SegmentedItem(
            index = 0,
            count = 1,
            headline = stringResource(R.string.feature_settings_export_logs),
            supporting = { Text(stringResource(R.string.feature_settings_export_logs_text)) },
            leading = { Icon(AdbIcons.Download, contentDescription = null, Modifier.size(20.dp)) },
            onClick = onExportLogs,
        )
    }

    SectionHeader(stringResource(R.string.feature_settings_about))
    SegmentedGroup(2) { index ->
        if (index == 0) {
            SegmentedItem(
                index = 0,
                count = 2,
                headline = stringResource(R.string.feature_settings_app_name),
                supporting = { Text(stringResource(R.string.feature_settings_app_version, appVersion)) },
                leading = { Icon(AdbIcons.Info, contentDescription = null, Modifier.size(20.dp)) },
            )
        } else {
            SegmentedItem(
                index = 1,
                count = 2,
                headline = stringResource(R.string.feature_settings_source),
                supporting = { Text(stringResource(R.string.feature_settings_source_text)) },
                leading = { Icon(AdbIcons.Code, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.OpenInNew, contentDescription = null) },
                onClick = onOpenSource,
            )
        }
    }
}
