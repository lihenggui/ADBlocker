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

package li.mercury.adblocker.core.designsystem.motion

import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.AnimationVector
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.TwoWayConverter
import androidx.compose.animation.core.VectorizedFiniteAnimationSpec
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MotionScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp

/*
 * Screen transitions. M3's transition patterns still use easing and duration
 * (MotionTokens), not springs: forward and backward slide a short distance
 * while fading, which is Android's way (iOS uses parallax); top-level
 * destinations fade out quickly, then in; peers such as tabs slide together
 * without fading.
 */

/** Emphasized easing, for transitions that stay on screen. */
val EmphasizedEasing = CubicBezierEasing(0.2f, 0f, 0f, 1f)

/** Emphasized decelerate, for what enters. */
val EmphasizedDecelerateEasing = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)

/** Emphasized accelerate, for what leaves. */
val EmphasizedAccelerateEasing = CubicBezierEasing(0.3f, 0f, 0.8f, 0.15f)

/** How far a screen slides going forward or back. */
val ScreenSlideDistance = 96.dp

private const val ENTER_MS = 350
private const val EXIT_MS = 200
private const val TOP_LEVEL_OUT_MS = 90
private const val TOP_LEVEL_IN_MS = 210
private const val LATERAL_MS = 300

/** The transitions between screens, with the slide distance in pixels for the current density. */
class ScreenTransitions internal constructor(distance: Int, isRtl: Boolean) {
    /** Going forward: the new screen slides in from the end while fading in; the old one leaves the other way. */
    val forward: ContentTransform
        get() = slide(forwardOffset)

    /** Going back: the mirror of [forward]. */
    val backward: ContentTransform
        get() = slide(backwardOffset)

    private val forwardOffset = enterOffset(distance, forward = true, isRtl)
    private val backwardOffset = enterOffset(distance, forward = false, isRtl)

    private fun slide(offset: Int): ContentTransform = (
        slideInHorizontally(tween(ENTER_MS, easing = EmphasizedDecelerateEasing)) { offset } +
            fadeIn(tween(ENTER_MS, easing = EmphasizedDecelerateEasing))
        ) togetherWith (
        slideOutHorizontally(tween(EXIT_MS, easing = EmphasizedAccelerateEasing)) { -offset } +
            fadeOut(tween(EXIT_MS, easing = EmphasizedAccelerateEasing))
        )

    /** Between top-level destinations: the old one fades out quickly, then the new one fades in, without sliding. */
    val topLevel: ContentTransform
        get() = fadeIn(tween(TOP_LEVEL_IN_MS, delayMillis = TOP_LEVEL_OUT_MS, easing = EmphasizedDecelerateEasing)) togetherWith
            fadeOut(tween(TOP_LEVEL_OUT_MS, easing = EmphasizedAccelerateEasing))
}

/** The screen transitions at this density and layout direction. */
@Composable
fun rememberScreenTransitions(): ScreenTransitions {
    val density = LocalDensity.current
    val isRtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    return remember(density, isRtl) { ScreenTransitions(with(density) { ScreenSlideDistance.roundToPx() }, isRtl) }
}

/** Peers, such as tabs: old and new slide together, the full width, without fading. */
fun lateralTransform(forward: Boolean, isRtl: Boolean): ContentTransform = slideInHorizontally(tween(LATERAL_MS, easing = EmphasizedEasing)) { enterOffset(it, forward, isRtl) } togetherWith
    slideOutHorizontally(tween(LATERAL_MS, easing = EmphasizedEasing)) { -enterOffset(it, forward, isRtl) }

internal fun enterOffset(distance: Int, forward: Boolean, isRtl: Boolean): Int = if (forward != isRtl) distance else -distance

/** How long what leaves a swap takes to fade before what replaces it starts to appear. */
const val SWAP_DELAY_MS = 100

/**
 * The content of a component changing (a title, a button): the old fades out
 * quickly, then the new rises a little into place and fades in, on the
 * motion scheme's springs.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
fun swapTransform(motion: MotionScheme, rise: Int): ContentTransform = (
    slideInVertically(motion.defaultSpatialSpec<IntOffset>().delayed(SWAP_DELAY_MS)) { rise } +
        fadeIn(motion.defaultEffectsSpec<Float>().delayed(SWAP_DELAY_MS))
    ) togetherWith fadeOut(motion.fastEffectsSpec())

/** This spec, started [delayMillis] later: springs have no delay of their own. */
fun <T> FiniteAnimationSpec<T>.delayed(delayMillis: Int): FiniteAnimationSpec<T> = DelayedSpec(this, delayMillis * 1_000_000L)

private class DelayedSpec<T>(private val spec: FiniteAnimationSpec<T>, private val delayNanos: Long) : FiniteAnimationSpec<T> {
    override fun <V : AnimationVector> vectorize(converter: TwoWayConverter<T, V>): VectorizedFiniteAnimationSpec<V> = DelayedVectorizedSpec(spec.vectorize(converter), delayNanos)

    override fun equals(other: Any?) = other is DelayedSpec<*> && other.spec == spec && other.delayNanos == delayNanos

    override fun hashCode() = 31 * spec.hashCode() + delayNanos.hashCode()
}

private class DelayedVectorizedSpec<V : AnimationVector>(private val spec: VectorizedFiniteAnimationSpec<V>, private val delayNanos: Long) : VectorizedFiniteAnimationSpec<V> {
    override fun getValueFromNanos(playTimeNanos: Long, initialValue: V, targetValue: V, initialVelocity: V): V = if (playTimeNanos < delayNanos) {
        initialValue
    } else {
        spec.getValueFromNanos(playTimeNanos - delayNanos, initialValue, targetValue, initialVelocity)
    }

    override fun getVelocityFromNanos(playTimeNanos: Long, initialValue: V, targetValue: V, initialVelocity: V): V = if (playTimeNanos < delayNanos) {
        initialVelocity
    } else {
        spec.getVelocityFromNanos(playTimeNanos - delayNanos, initialValue, targetValue, initialVelocity)
    }

    override fun getDurationNanos(initialValue: V, targetValue: V, initialVelocity: V): Long = delayNanos + spec.getDurationNanos(initialValue, targetValue, initialVelocity)
}
