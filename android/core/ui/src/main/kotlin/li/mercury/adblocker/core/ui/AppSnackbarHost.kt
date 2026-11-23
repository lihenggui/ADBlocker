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
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarDuration
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.AccessibilityManager
import androidx.compose.ui.platform.LocalAccessibilityManager
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.dismiss
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntSize
import kotlinx.coroutines.delay

/**
 * The snackbar, in the screen that [LocalHostsSnackbar] names, above its FAB
 * or toolbar.
 *
 * It enters by expanding upward from its bottom edge and leaves by fading:
 * M3's components expand and collapse along an axis, and avoid scaling,
 * which Compose's own host does. One without an action leaves after its
 * duration, lengthened as the accessibility settings ask; one with an action
 * stays until it is used or dismissed.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun AppSnackbarHost(modifier: Modifier = Modifier) {
    if (!LocalHostsSnackbar.current) return
    val state = LocalSnackbarHostState.current
    val current = state.currentSnackbarData
    val accessibility = LocalAccessibilityManager.current
    LaunchedEffect(current) {
        if (current != null) {
            delay(current.visuals.duration.millis(current.visuals.actionLabel != null, accessibility))
            current.dismiss()
        }
    }
    val motion = MaterialTheme.motionScheme
    AnimatedContent(
        targetState = current,
        modifier = modifier,
        contentAlignment = Alignment.BottomCenter,
        transitionSpec = {
            (expandVertically(motion.fastSpatialSpec<IntSize>(), expandFrom = Alignment.Bottom) + fadeIn(motion.fastEffectsSpec())) togetherWith
                fadeOut(motion.fastEffectsSpec()) using SizeTransform(clip = false)
        },
        label = "snackbar",
    ) { data ->
        if (data != null) {
            Snackbar(
                data,
                Modifier.semantics {
                    liveRegion = LiveRegionMode.Polite
                    dismiss {
                        data.dismiss()
                        true
                    }
                },
            )
        }
    }
}

private fun SnackbarDuration.millis(hasAction: Boolean, accessibility: AccessibilityManager?): Long {
    val original = when (this) {
        SnackbarDuration.Indefinite -> Long.MAX_VALUE
        SnackbarDuration.Long -> 10_000L
        SnackbarDuration.Short -> 4_000L
    }
    return accessibility?.calculateRecommendedTimeoutMillis(original, containsIcons = true, containsText = true, containsControls = hasAction) ?: original
}
