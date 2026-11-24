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

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.ui.AdaptiveSheet
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.profileAppLabel

/**
 * The hosts that apps would not accept our certificate for, by app, each to
 * be forgotten on its own, or all at once after a confirmation. They are read
 * from the daemon when shown, and never kept.
 */
@Composable
internal fun PinnedSheet(pinned: List<PinnedHost>, labels: Map<String, String>, onForget: (String, String?) -> Unit, onForgetAll: () -> Unit, onDismiss: () -> Unit) {
    var confirming by rememberSaveable { mutableStateOf(false) }
    AdaptiveSheet(onDismiss = onDismiss, title = stringResource(R.string.feature_settings_pinned)) {
        Column(Modifier.padding(horizontal = 16.dp)) {
            Text(
                stringResource(R.string.feature_settings_pinned_text),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 8.dp),
            )
            for ((appKey, hosts) in pinned.groupBy(PinnedHost::appKey)) {
                val app = hosts.first()
                val label = when {
                    app.packageName.startsWith('@') -> profileAppLabel(app.packageName.removePrefix("@"), app.profileSerial != null, sharedGroup = true)
                    app.profileSerial != null -> profileAppLabel(app.packageName, work = true)
                    else -> labels[app.packageName] ?: app.packageName
                }
                SectionHeader(label, trailing = hosts.size.toString())
                SegmentedGroup(hosts.size) { index ->
                    val host = hosts[index].host
                    SegmentedItem(
                        index = index,
                        count = hosts.size,
                        headline = host,
                        headlineStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
                        trailing = { TextButton(onClick = { onForget(appKey, host) }) { Text(stringResource(R.string.feature_settings_forget)) } },
                    )
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.End) {
                FilledTonalButton(onClick = { confirming = true }) { Text(stringResource(R.string.feature_settings_forget_all)) }
            }
        }
    }
    if (confirming) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text(stringResource(R.string.feature_settings_forget_all_title)) },
            text = { Text(stringResource(R.string.feature_settings_forget_all_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = false
                    onForgetAll()
                    onDismiss()
                }) { Text(stringResource(R.string.feature_settings_forget_all)) }
            },
            dismissButton = { TextButton(onClick = { confirming = false }) { Text(stringResource(R.string.feature_settings_cancel)) } },
        )
    }
}
