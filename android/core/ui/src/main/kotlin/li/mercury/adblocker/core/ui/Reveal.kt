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

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.updateTransition
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import li.mercury.adblocker.core.designsystem.icon.AdbIcons

/**
 * Shows [content] while [visible], growing it in from the top and fading it
 * in, and the reverse as it leaves, so that what is below makes room rather
 * than jumping.
 */
@Composable
fun Reveal(visible: Boolean, modifier: Modifier = Modifier, content: @Composable AnimatedVisibilityScope.() -> Unit) {
    val motion = MaterialTheme.motionScheme
    AnimatedVisibility(
        visible = visible,
        modifier = modifier,
        enter = expandVertically(motion.defaultSpatialSpec()) + fadeIn(motion.defaultEffectsSpec()),
        exit = shrinkVertically(motion.defaultSpatialSpec()) + fadeOut(motion.fastEffectsSpec()),
        content = content,
    )
}

/**
 * Shows [content] for [value] while there is one, as [Reveal] does. As it
 * leaves, it keeps showing the last value rather than collapsing empty.
 */
@Composable
fun <T : Any> RevealNotNull(value: T?, modifier: Modifier = Modifier, content: @Composable (T) -> Unit) {
    val motion = MaterialTheme.motionScheme
    val transition = updateTransition(value, label = "reveal")
    transition.AnimatedVisibility(
        visible = { it != null },
        modifier = modifier,
        enter = expandVertically(motion.defaultSpatialSpec()) + fadeIn(motion.defaultEffectsSpec()),
        exit = shrinkVertically(motion.defaultSpatialSpec()) + fadeOut(motion.fastEffectsSpec()),
    ) {
        (transition.targetState ?: transition.currentState)?.let { content(it) }
    }
}

/**
 * Shows [content] while [visible], scaling and fading it in where it
 * stands, for an icon or a button that comes and goes on its own.
 */
@Composable
fun Pop(visible: Boolean, modifier: Modifier = Modifier, content: @Composable AnimatedVisibilityScope.() -> Unit) {
    val motion = MaterialTheme.motionScheme
    AnimatedVisibility(
        visible = visible,
        modifier = modifier,
        enter = scaleIn(motion.fastSpatialSpec(), initialScale = POP_SCALE) + fadeIn(motion.fastEffectsSpec()),
        exit = scaleOut(motion.fastSpatialSpec(), targetScale = POP_SCALE) + fadeOut(motion.fastEffectsSpec()),
        content = content,
    )
}

/** How small what [Pop] shows starts and ends: small enough to read as arriving, not as flickering. */
private const val POP_SCALE = 0.6f

/** A chevron that turns to point up while [expanded], as what it opens grows in. */
@Composable
fun ExpandChevron(expanded: Boolean, modifier: Modifier = Modifier, tint: Color = LocalContentColor.current) {
    val turn = animateFloatAsState(if (expanded) 180f else 0f, MaterialTheme.motionScheme.fastSpatialSpec(), label = "turn")
    // Past half a turn it is the upward chevron, turned the rest of the way, so
    // that at rest it is drawn exactly as that symbol is. Composed again only
    // then: the turn itself is read while drawing.
    val up by remember { derivedStateOf { turn.value > HALF_TURN } }
    Icon(
        if (up) AdbIcons.ExpandLess else AdbIcons.ExpandMore,
        contentDescription = null,
        tint = tint,
        modifier = modifier.graphicsLayer { rotationZ = if (up) turn.value - 2 * HALF_TURN else turn.value },
    )
}

private const val HALF_TURN = 90f
