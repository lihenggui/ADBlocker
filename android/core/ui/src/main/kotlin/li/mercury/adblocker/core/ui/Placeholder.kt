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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.LineBreak
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

/**
 * What a pane shows when it has nothing to show yet: the detail pane before
 * anything is selected in the list beside it, or an empty list. The shape is
 * decoration, which is where the M3 shape guidance puts abstract shapes.
 * [prominent] is for a page whose main content is missing: larger art and
 * type, as on the setup pages. A [secondaryAction] goes below [action],
 * with less emphasis.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun Placeholder(
    icon: ImageVector,
    title: String,
    modifier: Modifier = Modifier,
    text: String? = null,
    action: (@Composable () -> Unit)? = null,
    secondaryAction: (@Composable () -> Unit)? = null,
    prominent: Boolean = false,
) {
    val type = MaterialTheme.typography
    Column(
        modifier = modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier.size(if (prominent) 128.dp else 72.dp).clip(MaterialShapes.Cookie9Sided.toShape())
                .background(if (prominent) MaterialTheme.colorScheme.tertiaryContainer else MaterialTheme.colorScheme.secondaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                icon,
                contentDescription = null,
                tint = if (prominent) MaterialTheme.colorScheme.onTertiaryContainer else MaterialTheme.colorScheme.onSecondaryContainer,
                modifier = Modifier.size(if (prominent) 56.dp else 24.dp),
            )
        }
        Spacer(Modifier.height(if (prominent) 24.dp else 12.dp))
        // A page's words keep a readable measure on wide windows.
        Column(
            if (prominent) Modifier.widthIn(max = 400.dp) else Modifier,
            verticalArrangement = Arrangement.spacedBy(if (prominent) 8.dp else 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            // Centred lines of even length leave no word alone on the last.
            Text(title, style = if (prominent) type.headlineSmallEmphasized.balanced() else type.titleMedium, textAlign = TextAlign.Center)
            if (text != null) {
                Text(
                    text,
                    style = if (prominent) type.bodyLarge.balanced() else type.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                )
            }
        }
        if (action != null || secondaryAction != null) {
            Spacer(Modifier.height(if (prominent) 24.dp else 12.dp))
            Column(verticalArrangement = Arrangement.spacedBy(8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                action?.invoke()
                secondaryAction?.invoke()
            }
        }
    }
}

private fun TextStyle.balanced() = copy(lineBreak = LineBreak(LineBreak.Strategy.Balanced, LineBreak.Strictness.Strict, LineBreak.WordBreak.Phrase))
