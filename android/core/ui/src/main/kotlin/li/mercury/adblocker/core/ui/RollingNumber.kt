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

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.IntOffset

/**
 * A number whose changed digits roll up, and only when it changes: nothing
 * moves while nothing happens. In tabular figures, so that the width does not
 * jump (M3 typography). Read as the whole number by screen readers.
 */
@Composable
fun RollingNumber(text: String, style: TextStyle, modifier: Modifier = Modifier) {
    val motion = MaterialTheme.motionScheme
    val tabular = style.copy(fontFeatureSettings = "tnum")
    Row(modifier.clearAndSetSemantics { contentDescription = text }) {
        text.forEachIndexed { index, char ->
            AnimatedContent(
                targetState = char,
                transitionSpec = {
                    // The new digit rises into its slot, which clips it; the old one rises out and fades.
                    slideInVertically(motion.fastSpatialSpec<IntOffset>()) { it } togetherWith
                        (slideOutVertically(motion.fastSpatialSpec<IntOffset>()) { -it } + fadeOut(motion.fastEffectsSpec()))
                },
                label = "digit $index",
            ) { digit -> Text(digit.toString(), style = tabular) }
        }
    }
}
