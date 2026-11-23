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

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The corners of a pane that holds content beside a list: extra large. */
private val PaneShape = RoundedCornerShape(28.dp)

/**
 * A pane beside the list of a list-detail layout: a container in surface
 * container low with extra large corners, clear of the window's edges by the
 * page margin (M3 panes), so that it reads as a place of its own. Within it,
 * surface is that container, so that the screen's bars and backgrounds sit
 * on it.
 *
 * The space on its start side is the scaffold's spacer, where the pane's
 * drag handle is.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun PaneSurface(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val paneScheme = remember(scheme) { scheme.copy(surface = scheme.surfaceContainerLow, background = scheme.surfaceContainerLow) }
    Box(
        modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.End + WindowInsetsSides.Bottom))
            .padding(end = PANE_MARGIN.dp, bottom = PANE_BOTTOM.dp),
    ) {
        Surface(shape = PaneShape, color = paneScheme.surface, modifier = Modifier.fillMaxSize()) {
            MaterialExpressiveTheme(colorScheme = paneScheme, content = content)
        }
    }
}

private const val PANE_MARGIN = 24
private const val PANE_BOTTOM = 8
