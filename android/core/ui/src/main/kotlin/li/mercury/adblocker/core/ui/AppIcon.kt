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

package li.mercury.adblocker.core.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import li.mercury.adblocker.core.model.InstalledApp

/**
 * An app's own icon, drawn the way the launcher draws it; a tile with the
 * name's first letter when the app is not installed. App icons keep the
 * system's shape, so that they stay recognizable: they are never put on the
 * app's own shapes.
 *
 * Icons load when shown, off the main thread, and stay in memory (Coil, with
 * [li.mercury.adblocker.core.ui.coil.AppIconFetcher]).
 */
@Composable
fun AppIcon(app: InstalledApp?, name: String, modifier: Modifier = Modifier, size: Dp = 40.dp) {
    val source = app?.icon
    var loaded by remember(source) { mutableStateOf(false) }
    Box(modifier.size(size)) {
        if (source == null || (source.work != null && !loaded)) {
            Box(
                modifier = Modifier.matchParentSize().clip(CircleShape).background(MaterialTheme.colorScheme.secondaryContainer),
                contentAlignment = Alignment.Center,
            ) {
                Text(name.take(1).uppercase(), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.onSecondaryContainer)
            }
        }
        if (source != null) {
            AsyncImage(
                model = source,
                contentDescription = null,
                modifier = Modifier.matchParentSize(),
                onSuccess = { loaded = true },
                onError = { loaded = false },
            )
        }
    }
}

@Composable
fun profileAppLabel(label: String, packageName: String, work: Boolean, sharedGroup: Boolean = false): String {
    if (!work && !sharedGroup) return label
    val argument = appLabelArgument(label, packageName)
    val name = if (sharedGroup) stringResource(R.string.core_ui_shared_app_label, argument) else argument
    return if (work) stringResource(R.string.core_ui_work_app_label, name) else name
}
