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

package li.mercury.adblocker.core.designsystem.component

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.toPath
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Matrix
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.Morph
import androidx.graphics.shapes.RoundedPolygon

/** What the status mark shows. */
enum class MarkState {
    /** Filtering: the barrier is down. */
    Filtering,

    /** Paused by the user: the barrier is up. */
    Paused,

    /** The filtering service is not running: up, and in the error colors. */
    Stopped,
}

/**
 * The app's status mark: the launcher icon's barrier, on the app's signature
 * shape. The barrier drops when filtering starts and rises when it stops; the
 * shape morphs from Sunny (filtering) to Circle (paused) or Square (stopped).
 *
 * Shape morphing communicates the change of state, which is what the M3 shape
 * guidance reserves it for; the shapes themselves carry no meaning that is not
 * also in the text next to the mark. The barrier drops on the fast spatial
 * spring, whose single bounce is how a real boom barrier lands, and rises on
 * the default one; the morph, which changes a large area, uses the slow
 * spatial spring, and colors the effects springs. With animations turned off
 * in the system settings, all of it changes at once.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun StatusMark(
    state: MarkState,
    modifier: Modifier = Modifier,
    size: Dp = 136.dp,
    colors: MarkColors = MarkDefaults.colors(state),
    /** Whether the barrier is down; by default, while filtering. Setup raises it on the filtering shape until it is done. */
    armDown: Boolean = state == MarkState.Filtering,
) {
    val motion = MaterialTheme.motionScheme

    // The morph runs from the shape shown last to the new one.
    var from by remember { mutableStateOf(state) }
    var to by remember { mutableStateOf(state) }
    val progress = remember { Animatable(1f) }
    LaunchedEffect(state) {
        if (state == to) return@LaunchedEffect
        from = to
        to = state
        progress.snapTo(0f)
        progress.animateTo(1f, motion.slowSpatialSpec())
    }
    val morph = remember(from, to) { Morph(shapeOf(from), shapeOf(to)) }

    val armAngle by animateFloatAsState(
        targetValue = if (armDown) ARM_DOWN else ARM_UP,
        animationSpec = if (armDown) motion.fastSpatialSpec() else motion.defaultSpatialSpec(),
        label = "arm",
    )
    val face by animateColorAsState(colors.face, motion.slowEffectsSpec(), label = "face")
    val eye by animateColorAsState(colors.eye, motion.slowEffectsSpec(), label = "eye")
    val arm by animateColorAsState(colors.arm, motion.slowEffectsSpec(), label = "arm color")
    val stripe by animateColorAsState(colors.stripe, motion.slowEffectsSpec(), label = "stripe")
    val bolt by animateColorAsState(colors.bolt, motion.slowEffectsSpec(), label = "bolt")
    val post by animateColorAsState(colors.post, motion.slowEffectsSpec(), label = "post")

    val shapePath = remember { Path() }
    val scaled = remember { Path() }
    Box(modifier.size(size)) {
        Canvas(Modifier.fillMaxSize()) {
            // Shapes that are all symmetric under a quarter turn: turning the
            // morph by what is left of one lands it where it started to look.
            val turn = 90f * (1f - progress.value)
            morph.toPath(progress.value, shapePath)
            scaled.reset()
            scaled.addPath(shapePath)
            scaled.transform(Matrix().apply { scale(this@Canvas.size.width, this@Canvas.size.height) })
            rotate(turn) { drawPath(scaled, face) }
            drawBarrier(armAngle, eye = eye, arm = arm, stripe = stripe, hub = arm, bolt = bolt, post = post)
        }
    }
}

/** The colors of the mark's parts. */
data class MarkColors(
    val face: Color,
    val eye: Color,
    val arm: Color,
    val stripe: Color,
    val bolt: Color,
    val post: Color,
)

object MarkDefaults {
    /**
     * The mark as an illustration rather than a status (setup): on the
     * primary container, the barrier in the surface color.
     */
    @Composable
    fun featuredColors(): MarkColors {
        val scheme = MaterialTheme.colorScheme
        return MarkColors(
            face = scheme.primaryContainer,
            eye = scheme.onPrimaryContainer,
            arm = scheme.surface,
            stripe = scheme.tertiary,
            bolt = scheme.onPrimaryContainer,
            post = scheme.primary,
        )
    }

    @Composable
    fun colors(state: MarkState): MarkColors {
        val scheme = MaterialTheme.colorScheme
        return when (state) {
            MarkState.Filtering -> MarkColors(
                face = scheme.primary,
                eye = scheme.primaryContainer,
                arm = scheme.onPrimary,
                stripe = scheme.tertiary,
                bolt = scheme.primary,
                post = scheme.primaryContainer,
            )

            MarkState.Paused -> MarkColors(
                face = scheme.surfaceContainerHighest,
                eye = scheme.outline,
                arm = scheme.surface,
                stripe = scheme.outline,
                bolt = scheme.outline,
                post = scheme.outlineVariant,
            )

            MarkState.Stopped -> MarkColors(
                face = scheme.errorContainer,
                eye = scheme.onErrorContainer,
                arm = scheme.surface,
                stripe = scheme.onErrorContainer,
                bolt = scheme.onErrorContainer,
                post = scheme.surface,
            )
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
private fun shapeOf(state: MarkState): RoundedPolygon = when (state) {
    MarkState.Filtering -> MaterialShapes.Sunny
    MarkState.Paused -> MaterialShapes.Circle
    MarkState.Stopped -> MaterialShapes.Square
}

/** The barrier is drawn on the launcher icon's 108-unit grid; the arm turns about its hub. */
private const val GRID = 108f
private const val ARM_DOWN = 0f
private const val ARM_UP = -62f
private val HUB = Offset(31f, 65.5f)

/**
 * The barrier as on the launcher icon, scaled to fill 3/4 of the mark: the
 * eyes, the post, the striped arm turned [armAngle] degrees about the hub,
 * and the hub.
 */
private fun DrawScope.drawBarrier(armAngle: Float, eye: Color, arm: Color, stripe: Color, hub: Color, bolt: Color, post: Color) {
    val k = size.minDimension * 0.76f / GRID
    val inset = (size.minDimension - GRID * k) / 2f
    translate(inset, inset) {
        scale(k, k, pivot = Offset.Zero) {
            drawCircle(eye, radius = 6f, center = Offset(40f, 40f))
            drawCircle(eye, radius = 6f, center = Offset(68f, 40f))
            drawRoundRect(post, topLeft = Offset(25.5f, 66f), size = Size(11f, 30f), cornerRadius = CornerRadius(3f))
            rotate(armAngle, pivot = HUB) {
                val bar = Path().apply {
                    addRoundRect(androidx.compose.ui.geometry.RoundRect(25f, 59f, 95f, 72f, CornerRadius(6.5f)))
                }
                drawPath(bar, arm)
                clipPath(bar) {
                    for (x in listOf(42f, 58f, 74f)) {
                        val stripePath = Path().apply {
                            moveTo(x, 59f)
                            lineTo(x + 7.5f, 59f)
                            lineTo(x + 11.8f, 72f)
                            lineTo(x + 4.3f, 72f)
                            close()
                        }
                        drawPath(stripePath, stripe)
                    }
                }
            }
            drawCircle(hub, radius = 8.5f, center = HUB)
            drawCircle(bolt, radius = 3f, center = HUB)
        }
    }
}
