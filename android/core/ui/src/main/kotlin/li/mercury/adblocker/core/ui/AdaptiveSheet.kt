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

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.movableContentOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

/**
 * Supplementary content over the screen: a modal bottom sheet in compact and
 * medium windows, a dialog in expanded and wider ones, where M3 swaps bottom
 * sheets out. The sheet opens fully, slides rather than fades, and has the
 * drag handle; both close on a tap outside and on back.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AdaptiveSheet(onDismiss: () -> Unit, title: String, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    // One body for both containers, so that what it remembers survives the
    // window crossing the expanded breakpoint while it is open.
    val body = remember(content) {
        movableContentOf { modifier: Modifier, heading: String ->
            Column(modifier) {
                Text(heading, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(horizontal = 24.dp).padding(bottom = 16.dp))
                content()
            }
        }
    }
    if (LocalWideWindow.current) {
        Dialog(onDismissRequest = onDismiss, properties = DialogProperties(dismissOnBackPress = false)) {
            val progress = rememberPredictiveDismissProgress(onDismiss)
            Surface(
                shape = RoundedCornerShape(28.dp),
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                modifier = modifier.widthIn(min = 280.dp, max = 560.dp).graphicsLayer {
                    val fraction = PredictiveBackEasing.transform(progress.value)
                    scaleX = 1f - 0.1f * fraction
                    scaleY = scaleX
                },
            ) {
                body(Modifier.verticalScroll(rememberScrollState()).padding(vertical = 24.dp), title)
            }
        }
    } else {
        val sheetState = rememberBottomSheetState(
            initialValue = SheetValue.Hidden,
            enabledValues = setOf(SheetValue.Hidden, SheetValue.Expanded),
        )
        ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, modifier = modifier) {
            body(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = 16.dp), title)
        }
    }
}

private val PredictiveBackEasing = CubicBezierEasing(0.1f, 0.1f, 0f, 1f)
