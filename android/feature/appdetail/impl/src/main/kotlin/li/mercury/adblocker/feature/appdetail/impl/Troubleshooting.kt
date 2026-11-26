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

package li.mercury.adblocker.feature.appdetail.impl

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.ExpandChevron
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.formatClockTime

/** What to do when the app misbehaves while filtered, from the quickest fix to the most precise. */
@Composable
internal fun Trouble(
    state: AppDetailUiState,
    entry: AppEntry,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onTogglePins: () -> Unit,
    onForget: (String?) -> Unit,
    onOpenActivity: () -> Unit,
) {
    val pausedUntil = entry.filter?.pausedUntil?.takeIf { entry.isFiltered }
    SegmentedGroup(3) { index ->
        when (index) {
            0 -> SegmentedItem(
                index = 0,
                count = 3,
                headline = stringResource(
                    when {
                        pausedUntil != null -> R.string.feature_appdetail_resume
                        entry.app.isSharedGroup -> R.string.feature_appdetail_pause_group
                        else -> R.string.feature_appdetail_pause
                    },
                ),
                supporting = {
                    Text(
                        if (pausedUntil != null) {
                            stringResource(R.string.feature_appdetail_paused_until, formatClockTime(LocalContext.current, pausedUntil))
                        } else {
                            stringResource(R.string.feature_appdetail_pause_text)
                        },
                    )
                },
                leading = { Icon(if (pausedUntil != null) AdbIcons.PlayArrow else AdbIcons.Timer, contentDescription = null, Modifier.size(20.dp)) },
                onClick = if (entry.isFiltered) {
                    if (pausedUntil != null) onResume else onPause
                } else {
                    null
                },
            )

            1 -> SegmentedItem(
                index = 1,
                count = 3,
                headline = stringResource(R.string.feature_appdetail_requests),
                supporting = {
                    Text(stringResource(if (entry.app.isSharedGroup) R.string.feature_appdetail_group_requests_text else R.string.feature_appdetail_requests_text))
                },
                leading = { Icon(AdbIcons.ReceiptLong, contentDescription = null, Modifier.size(20.dp)) },
                trailing = { Icon(AdbIcons.ChevronRight, contentDescription = null) },
                onClick = onOpenActivity,
            )

            else -> PinnedHostsTroubleshooting(state.pinned, state.pinnedExpanded, onTogglePins, onForget)
        }
    }
}

@Composable
private fun PinnedHostsTroubleshooting(
    pins: ReadState<List<PinnedHost>>,
    expanded: Boolean,
    onTogglePins: () -> Unit,
    onForget: (String?) -> Unit,
) {
    Column {
        val pinned = pins.valueOrNull.orEmpty()
        SegmentedItem(
            index = 2,
            count = 3,
            headline = stringResource(R.string.feature_appdetail_pinned),
            supporting = {
                Text(
                    when (pins) {
                        ReadState.Pending -> stringResource(R.string.feature_appdetail_pinned_checking)

                        is ReadState.Unavailable -> stringResource(R.string.feature_appdetail_pinned_unavailable)

                        is ReadState.Available -> if (pinned.isEmpty()) {
                            stringResource(R.string.feature_appdetail_pinned_none)
                        } else {
                            stringResource(R.string.feature_appdetail_pinned_count, pinned.size)
                        }
                    },
                )
            },
            leading = { Icon(AdbIcons.Lock, contentDescription = null, Modifier.size(20.dp)) },
            trailing = if (pinned.isNotEmpty()) {
                { ExpandChevron(expanded) }
            } else {
                null
            },
            onClick = if (pinned.isNotEmpty()) onTogglePins else null,
        )
        PinnedList(pinned, expanded, onForget)
    }
}

@Composable
private fun PinnedList(pinned: List<PinnedHost>, expanded: Boolean, onForget: (String?) -> Unit) {
    // Expands along y, as a list does (M3 lists: expand and collapse), and keeps the hosts while it folds.
    RevealNotNull(pinned.takeIf { expanded && it.isNotEmpty() }) { pinned ->
        // A host forgotten leaves as the ones below move up.
        Column(Modifier.animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec()).padding(start = 48.dp, top = 4.dp)) {
            Text(
                stringResource(R.string.feature_appdetail_pinned_text),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(end = 16.dp, bottom = 4.dp),
            )
            pinned.forEach { host ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(host.host, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace), modifier = Modifier.weight(1f))
                    TextButton(onClick = { onForget(host.host) }) { Text(stringResource(R.string.feature_appdetail_forget)) }
                }
            }
            TextButton(onClick = { onForget(null) }) { Text(stringResource(R.string.feature_appdetail_forget_all)) }
        }
    }
}
