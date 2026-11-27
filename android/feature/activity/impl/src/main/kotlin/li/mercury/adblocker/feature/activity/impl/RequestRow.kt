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

package li.mercury.adblocker.feature.activity.impl

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.formatClockTime
import li.mercury.adblocker.core.ui.profileAppLabel
import li.mercury.adblocker.core.ui.segmentedColors

// Recorded request rows and the labels they share with the detail screen.

internal enum class RequestRowPosition(val index: Int, val count: Int) {
    Only(0, 1),
    First(0, 2),
    Middle(1, 3),
    Last(1, 2),
    ;

    companion object {
        fun of(index: Int, count: Int): RequestRowPosition = when {
            count == 1 -> Only
            index == 0 -> First
            index == count - 1 -> Last
            else -> Middle
        }
    }
}

/** One request: the app and host first, then what became of it, in words and an icon as well as color. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun RequestRow(
    event: RequestEvent,
    app: InstalledApp?,
    position: RequestRowPosition,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val (icon, label, color) = verdictLook(event.verdict)
    Box(modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            selected = selected && LocalShowsDetailBesideList.current,
            onClick = onClick,
            shapes = ListItemDefaults.segmentedShapes(position.index, position.count),
            colors = segmentedColors(),
            leadingContent = { AppIcon(app, app?.label ?: event.packageName) },
            supportingContent = {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(icon, contentDescription = null, Modifier.size(16.dp), tint = color)
                    Text(stringResource(label), color = color, style = MaterialTheme.typography.bodyMedium, maxLines = 1)
                    // The app and the kind give way first: the verdict is what the row is for.
                    Text(
                        "· " + profileAppLabel(app?.label ?: event.packageName.removePrefix("@"), event.packageName, event.profileSerial != null, event.packageName.startsWith('@')) + (event.kind?.let { " · " + kindName(it) } ?: ""),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
            },
            trailingContent = { Text(formatClockTime(LocalContext.current, event.time, includeSeconds = true), style = MaterialTheme.typography.labelMedium.copy(fontFeatureSettings = "tnum")) },
            content = {
                Text(event.host, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace))
            },
        )
    }
}

@Composable
internal fun verdictLook(verdict: Verdict) = when (verdict) {
    Verdict.Blocked -> Triple(AdbIcons.Block, R.string.feature_activity_verdict_blocked, MaterialTheme.colorScheme.primary)
    Verdict.HostBlocked -> Triple(AdbIcons.Block, R.string.feature_activity_verdict_host_blocked, MaterialTheme.colorScheme.primary)
    Verdict.Allowed -> Triple(AdbIcons.Check, R.string.feature_activity_verdict_allowed, MaterialTheme.colorScheme.onSurfaceVariant)
    Verdict.Passed -> Triple(AdbIcons.Lock, R.string.feature_activity_verdict_passed, MaterialTheme.colorScheme.tertiary)
}

@Composable
internal fun kindName(kind: String) = when (kind) {
    "script" -> stringResource(R.string.feature_activity_kind_script)
    "image" -> stringResource(R.string.feature_activity_kind_image)
    "document", "subdocument" -> stringResource(R.string.feature_activity_kind_document)
    "stylesheet" -> stringResource(R.string.feature_activity_kind_stylesheet)
    "font" -> stringResource(R.string.feature_activity_kind_font)
    "media" -> stringResource(R.string.feature_activity_kind_media)
    "xmlhttprequest" -> "XHR"
    else -> kind
}
