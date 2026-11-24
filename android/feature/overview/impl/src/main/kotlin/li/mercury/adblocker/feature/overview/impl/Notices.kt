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

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonColors
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.ui.R as UiR

/** What a notice's buttons do. */
internal class NoticeActions(
    val dismiss: (OverviewNotice) -> Unit,
    val switchToRoot: () -> Unit,
    val batteryExemption: () -> Unit,
    val downloadLists: () -> Unit = {},
    val openLists: () -> Unit = {},
    val allowAppList: () -> Unit = {},
)

/**
 * What to know about now: what it is, what it means, and what can be done.
 * Tips are in the secondary container. Apps or lists that are not filtered
 * are a state that needs attention: in the tertiary container while waiting
 * for a download or for the app list, in the error container once a
 * download failed.
 */
@Composable
internal fun Notice(notice: OverviewNotice, missing: MissingLists?, actions: NoticeActions) {
    val failed = notice == OverviewNotice.ListsMissing && missing?.failed == true
    val state = notice == OverviewNotice.ListsMissing || notice == OverviewNotice.AppListWithheld
    val scheme = MaterialTheme.colorScheme
    val (container, content, accent) = when {
        !state -> Triple(scheme.secondaryContainer, scheme.onSecondaryContainer, scheme.secondary to scheme.onSecondary)
        failed -> Triple(scheme.errorContainer, scheme.onErrorContainer, scheme.error to scheme.onError)
        else -> Triple(scheme.tertiaryContainer, scheme.onTertiaryContainer, scheme.tertiary to scheme.onTertiary)
    }
    Card(
        colors = CardDefaults.cardColors(containerColor = container, contentColor = content),
        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Icon(
                    when {
                        notice == OverviewNotice.Lockdown -> AdbIcons.Warning
                        notice == OverviewNotice.AppListWithheld -> AdbIcons.Apps
                        !state -> AdbIcons.Security
                        failed -> AdbIcons.Error
                        else -> AdbIcons.Download
                    },
                    contentDescription = null,
                    modifier = Modifier.padding(top = 2.dp).size(20.dp),
                )
                Column(Modifier.weight(1f)) {
                    Text(noticeTitle(notice, missing), style = MaterialTheme.typography.titleMedium)
                    Text(noticeText(notice, missing), style = MaterialTheme.typography.bodyMedium)
                }
            }
            NoticeButtons(notice, actions, ButtonDefaults.textButtonColors(contentColor = content), ButtonDefaults.buttonColors(containerColor = accent.first, contentColor = accent.second))
        }
    }
}

@Composable
private fun noticeTitle(notice: OverviewNotice, missing: MissingLists?): String = when (notice) {
    OverviewNotice.ListsMissing -> {
        val count = missing?.count ?: 0
        val title = if (missing?.failed == true) R.plurals.feature_overview_lists_failed_title else R.plurals.feature_overview_lists_pending_title
        pluralStringResource(title, count, count)
    }

    OverviewNotice.AppListWithheld -> stringResource(R.string.feature_overview_app_list_withheld_title)

    OverviewNotice.Lockdown -> stringResource(R.string.feature_overview_lockdown_title)

    OverviewNotice.RootAvailable -> stringResource(R.string.feature_overview_root_title)

    OverviewNotice.BatteryExemption -> stringResource(R.string.feature_overview_battery_title)
}

@Composable
private fun noticeText(notice: OverviewNotice, missing: MissingLists?): String = when (notice) {
    OverviewNotice.ListsMissing -> pluralStringResource(
        if (missing?.failed == true) R.plurals.feature_overview_lists_missing_failed else R.plurals.feature_overview_lists_missing_pending,
        missing?.count ?: 0,
    )

    OverviewNotice.AppListWithheld -> stringResource(R.string.feature_overview_app_list_withheld_text)

    OverviewNotice.Lockdown -> stringResource(R.string.feature_overview_lockdown_text)

    OverviewNotice.RootAvailable -> stringResource(R.string.feature_overview_root_text)

    OverviewNotice.BatteryExemption -> stringResource(R.string.feature_overview_battery_text)
}

@Composable
private fun NoticeButtons(notice: OverviewNotice, actions: NoticeActions, textColors: ButtonColors, filledColors: ButtonColors) = Row(
    Modifier.fillMaxWidth().padding(top = 12.dp),
    horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
) {
    when (notice) {
        // Not dismissible: it goes once the lists have rules.
        OverviewNotice.ListsMissing -> {
            TextButton(onClick = actions.openLists, colors = textColors) { Text(stringResource(R.string.feature_overview_lists_missing_open)) }
            Button(onClick = actions.downloadLists, colors = filledColors) { Text(stringResource(R.string.feature_overview_lists_missing_download)) }
        }

        // Not dismissible either: it goes once the system shows the apps.
        OverviewNotice.AppListWithheld -> Button(onClick = actions.allowAppList, colors = filledColors) {
            Text(stringResource(UiR.string.core_ui_app_list_allow))
        }

        OverviewNotice.Lockdown -> TextButton(onClick = { actions.dismiss(notice) }, colors = textColors) {
            Text(stringResource(R.string.feature_overview_notice_ok))
        }

        OverviewNotice.RootAvailable, OverviewNotice.BatteryExemption -> {
            val battery = notice == OverviewNotice.BatteryExemption
            TextButton(onClick = { actions.dismiss(notice) }, colors = textColors) { Text(stringResource(R.string.feature_overview_root_dismiss)) }
            Button(onClick = if (battery) actions.batteryExemption else actions.switchToRoot, colors = filledColors) {
                Text(stringResource(if (battery) R.string.feature_overview_battery_action else R.string.feature_overview_root_switch))
            }
        }
    }
}
