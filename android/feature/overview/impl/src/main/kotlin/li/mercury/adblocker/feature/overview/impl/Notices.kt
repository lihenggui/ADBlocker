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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons

/** What a notice's buttons do. */
internal class NoticeActions(
    val dismiss: (OverviewNotice) -> Unit,
    val switchToRoot: () -> Unit,
    val batteryExemption: () -> Unit,
)

/** Something to know once, in the secondary container: what it is, what it means, and what can be done. */
@Composable
internal fun Notice(notice: OverviewNotice, actions: NoticeActions) {
    Card(
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.secondaryContainer,
            contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        ),
        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
    ) {
        Column(Modifier.padding(16.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Icon(
                    if (notice == OverviewNotice.Lockdown) AdbIcons.Warning else AdbIcons.Security,
                    contentDescription = null,
                    modifier = Modifier.padding(top = 2.dp).size(20.dp),
                )
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(
                            when (notice) {
                                OverviewNotice.Lockdown -> R.string.feature_overview_lockdown_title
                                OverviewNotice.RootAvailable -> R.string.feature_overview_root_title
                                OverviewNotice.BatteryExemption -> R.string.feature_overview_battery_title
                            },
                        ),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        stringResource(
                            when (notice) {
                                OverviewNotice.Lockdown -> R.string.feature_overview_lockdown_text
                                OverviewNotice.RootAvailable -> R.string.feature_overview_root_text
                                OverviewNotice.BatteryExemption -> R.string.feature_overview_battery_text
                            },
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                val textColors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSecondaryContainer)
                when (notice) {
                    OverviewNotice.Lockdown -> TextButton(onClick = { actions.dismiss(notice) }, colors = textColors) {
                        Text(stringResource(R.string.feature_overview_notice_ok))
                    }

                    OverviewNotice.RootAvailable, OverviewNotice.BatteryExemption -> {
                        val battery = notice == OverviewNotice.BatteryExemption
                        TextButton(onClick = { actions.dismiss(notice) }, colors = textColors) { Text(stringResource(R.string.feature_overview_root_dismiss)) }
                        Button(
                            onClick = if (battery) actions.batteryExemption else actions.switchToRoot,
                            colors = ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.secondary,
                                contentColor = MaterialTheme.colorScheme.onSecondary,
                            ),
                        ) { Text(stringResource(if (battery) R.string.feature_overview_battery_action else R.string.feature_overview_root_switch)) }
                    }
                }
            }
        }
    }
}
