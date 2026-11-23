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

package li.mercury.adblocker.feature.onboarding.impl.steps

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ContainedLoadingIndicator
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.DeviceCheckKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepActions
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.segmentedColors
import li.mercury.adblocker.feature.onboarding.impl.OnboardingUiState
import li.mercury.adblocker.feature.onboarding.impl.R

/** What the check's art shows: waiting, done, failed, or root missing with the VPN to go on with. */
private enum class CheckArt { Checking, Passed, Failed, Choice }

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.Check(state: OnboardingUiState, onNext: () -> Unit, onRetry: () -> Unit) {
    val motion = MaterialTheme.motionScheme
    val art = when {
        state.checking -> CheckArt.Checking
        state.checksPassed -> CheckArt.Passed
        state.vpnPossible -> CheckArt.Choice
        else -> CheckArt.Failed
    }
    // Without root the VPN can still filter: not an error, but a choice.
    val noRoot = state.checks[DeviceCheckKind.Root]?.passed == false
    StepPage(
        title = stringResource(R.string.feature_onboarding_check_title),
        text = stringResource(
            when {
                art != CheckArt.Choice -> R.string.feature_onboarding_check_text
                noRoot -> R.string.feature_onboarding_check_no_root_text
                else -> R.string.feature_onboarding_check_unsupported_text
            },
        ),
        art = {
            // The wait is short, under five seconds: a loading indicator (M3).
            // Done, its container keeps its place and shows the result.
            AnimatedContent(
                targetState = art,
                transitionSpec = {
                    (scaleIn(motion.fastSpatialSpec(), initialScale = 0.85f) + fadeIn(motion.fastEffectsSpec())) togetherWith fadeOut(motion.fastEffectsSpec())
                },
                contentAlignment = Alignment.Center,
                label = "check",
            ) { shown ->
                when (shown) {
                    CheckArt.Checking -> ContainedLoadingIndicator(Modifier.size(96.dp))
                    CheckArt.Passed -> CheckBadge(AdbIcons.Check, MaterialTheme.colorScheme.primaryContainer, MaterialTheme.colorScheme.onPrimaryContainer)
                    CheckArt.Failed -> CheckBadge(AdbIcons.Error, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer)
                    CheckArt.Choice -> CheckBadge(AdbIcons.VpnKey, MaterialTheme.colorScheme.secondaryContainer, MaterialTheme.colorScheme.onSecondaryContainer)
                }
            }
        },
        action = {
            FadeThrough(art, { it == CheckArt.Failed || it == CheckArt.Choice }, label = "check action") { art ->
                when (art) {
                    CheckArt.Failed -> StepAction(stringResource(R.string.feature_onboarding_check_again), onRetry)

                    CheckArt.Choice -> StepActions(
                        stringResource(if (noRoot) R.string.feature_onboarding_check_root_again else R.string.feature_onboarding_check_again),
                        onRetry,
                        stringResource(R.string.feature_onboarding_continue),
                        onNext,
                    )

                    else -> StepAction(stringResource(R.string.feature_onboarding_continue), onNext, enabled = state.checksPassed)
                }
            }
        },
    ) {
        CheckRows(state, art, noRoot)
    }
}

@Composable
private fun CheckRows(state: OnboardingUiState, art: CheckArt, noRoot: Boolean) {
    // Without root, what only the root module needs is left out: the
    // kernel cannot be read, and the VPN runs from the app's own minimum.
    val shown = if (art == CheckArt.Choice && noRoot) {
        DeviceCheckKind.entries - DeviceCheckKind.Kernel
    } else {
        DeviceCheckKind.entries
    }
    // One check at a time, root first: the first one not done is the one under way.
    val current = DeviceCheckKind.entries.firstOrNull { it !in state.checks }
    SegmentedGroup(shown.size) { index ->
        val kind = shown[index]
        val check = state.checks[kind]
        val unknown = check != null && !check.passed && kind == DeviceCheckKind.Kernel && noRoot
        val neutral = art == CheckArt.Choice && noRoot
        // For the VPN, Android 10 is enough: the app runs on nothing older.
        val passed = check != null && (check.passed || (neutral && kind == DeviceCheckKind.AndroidVersion))
        val verifyAtStartup = passed && kind == DeviceCheckKind.Kernel && check.detail == null
        SegmentedItem(
            index = index,
            count = shown.size,
            headline = stringResource(checkTitle(kind)),
            supporting = {
                Text(
                    when {
                        check == null -> stringResource(R.string.feature_onboarding_checking)
                        verifyAtStartup -> stringResource(R.string.feature_onboarding_check_kernel_unverified)
                        passed -> passedDetail(kind, check.detail)
                        neutral && kind == DeviceCheckKind.Root -> stringResource(R.string.feature_onboarding_check_root_not_detected)
                        unknown -> stringResource(R.string.feature_onboarding_check_after_root)
                        else -> stringResource(checkFailure(kind))
                    },
                )
            },
            leading = {
                val mark = when {
                    check == null && kind == current -> CheckMark.Running
                    check == null -> CheckMark.Waiting
                    verifyAtStartup -> CheckMark.Note
                    passed -> CheckMark.Passed
                    neutral && kind == DeviceCheckKind.Root -> CheckMark.Note
                    unknown -> CheckMark.Later
                    else -> CheckMark.Failed
                }
                CheckMarkIcon(mark)
            },
        )
    }
    Reveal(art == CheckArt.Choice) { Column { MethodChoice(noRoot) } }
}

/** Where a check is, at the start of its row. */
private enum class CheckMark { Running, Waiting, Note, Passed, Later, Failed }

/** The mark of a check, fading through to the next as the check moves on. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CheckMarkIcon(mark: CheckMark) {
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        FadeThrough(mark, { it }, contentAlignment = Alignment.Center, label = "check mark") { mark ->
            when (mark) {
                // Each check takes a moment, under five seconds: the loading indicator (M3).
                CheckMark.Running -> LoadingIndicator(Modifier.size(24.dp))

                CheckMark.Waiting -> Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.outlineVariant))

                CheckMark.Note -> Icon(AdbIcons.Info, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)

                CheckMark.Passed -> Icon(AdbIcons.CheckCircleFilled, contentDescription = null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)

                CheckMark.Later -> Icon(AdbIcons.Schedule, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)

                CheckMark.Failed -> Icon(AdbIcons.Error, contentDescription = null, tint = MaterialTheme.colorScheme.error)
            }
        }
    }
}

@Composable
private fun CheckBadge(icon: ImageVector, container: Color, content: Color) {
    Box(Modifier.size(96.dp).clip(CircleShape).background(container), contentAlignment = Alignment.Center) {
        Icon(icon, contentDescription = null, tint = content, modifier = Modifier.size(48.dp))
    }
}

/**
 * The two methods, with the VPN chosen: the root module cannot run here, and
 * says why. Checking again offers it once root is granted.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun MethodChoice(noRoot: Boolean) {
    SectionHeader(stringResource(R.string.feature_onboarding_method))
    val options = listOf(
        Triple(R.string.feature_onboarding_method_vpn, R.string.feature_onboarding_method_vpn_text, true),
        Triple(
            R.string.feature_onboarding_method_root,
            if (noRoot) R.string.feature_onboarding_method_root_no_root else R.string.feature_onboarding_method_root_unsupported,
            false,
        ),
    )
    SegmentedGroup(options.size, Modifier.selectableGroup()) { index ->
        val (title, text, available) = options[index]
        SegmentedListItem(
            selected = available,
            onClick = {},
            enabled = available,
            shapes = ListItemDefaults.segmentedShapes(index, options.size),
            colors = segmentedColors(),
            leadingContent = { RadioButton(selected = available, onClick = null, enabled = available) },
            supportingContent = { Text(stringResource(text)) },
        ) { Text(stringResource(title)) }
    }
}

/** What a passed check found, in words. */
@Composable
private fun passedDetail(kind: DeviceCheckKind, detail: String?): String = when (kind) {
    DeviceCheckKind.Root -> detail?.let { stringResource(R.string.feature_onboarding_check_root_granted, it) } ?: stringResource(R.string.feature_onboarding_check_root_granted_plain)
    DeviceCheckKind.Kernel -> stringResource(R.string.feature_onboarding_check_kernel_ok)
    else -> detail ?: stringResource(R.string.feature_onboarding_ok)
}

private fun checkTitle(kind: DeviceCheckKind) = when (kind) {
    DeviceCheckKind.Root -> R.string.feature_onboarding_check_root
    DeviceCheckKind.AndroidVersion -> R.string.feature_onboarding_check_android
    DeviceCheckKind.Kernel -> R.string.feature_onboarding_check_kernel
    DeviceCheckKind.Architecture -> R.string.feature_onboarding_check_arch
}

private fun checkFailure(kind: DeviceCheckKind) = when (kind) {
    DeviceCheckKind.Root -> R.string.feature_onboarding_check_root_failed
    DeviceCheckKind.AndroidVersion -> R.string.feature_onboarding_check_android_failed
    DeviceCheckKind.Kernel -> R.string.feature_onboarding_check_kernel_failed
    DeviceCheckKind.Architecture -> R.string.feature_onboarding_check_arch_failed
}
