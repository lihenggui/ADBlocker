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

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.component.MarkDefaults
import li.mercury.adblocker.core.designsystem.component.MarkState
import li.mercury.adblocker.core.designsystem.component.StatusMark
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.failureText
import li.mercury.adblocker.feature.onboarding.impl.InstallStage
import li.mercury.adblocker.feature.onboarding.impl.InstallState
import li.mercury.adblocker.feature.onboarding.impl.OnboardingUiState
import li.mercury.adblocker.feature.onboarding.impl.R

/** Where each part of setting up is. */
internal enum class StepState { Waiting, Running, Done, Skipped }

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.Install(state: OnboardingUiState, onRetry: () -> Unit, onFinish: () -> Unit) {
    val install = state.install
    val lists = state.lists.count { it.value }
    StepPage(
        title = when (install) {
            is InstallState.Done -> pluralStringResource(R.plurals.feature_onboarding_done_title, install.apps, install.apps)
            is InstallState.Failed -> stringResource(R.string.feature_onboarding_failed_title)
            else -> stringResource(R.string.feature_onboarding_installing_title)
        },
        text = when (install) {
            is InstallState.Done -> stringResource(R.string.feature_onboarding_done_text)

            // The app's own words for what went wrong; the installer's below them.
            // Setup installs this app's root mode or keeps one of its version, so only an older
            // module that a root manager holds until a restart rejects a command as outdated.
            is InstallState.Failed -> when (install.failure) {
                LoadFailure.Failed -> stringResource(R.string.feature_onboarding_failed_text)
                LoadFailure.ModuleOutdated -> stringResource(R.string.feature_onboarding_restart_to_update)
                else -> failureText(install.failure)
            }

            else -> null
        },
        // The barrier is up while setting up, and drops once filtering starts.
        art = { StatusMark(MarkState.Filtering, size = 200.dp, colors = MarkDefaults.featuredColors(), armDown = install is InstallState.Done) },
        action = {
            FadeThrough(install, ::installActions, label = "install action") { install ->
                when {
                    install is InstallState.Done -> StepAction(stringResource(R.string.feature_onboarding_finish), onFinish)
                    install is InstallState.Failed -> StepAction(stringResource(R.string.feature_onboarding_try_again), onRetry)
                    else -> StepAction(stringResource(R.string.feature_onboarding_finish), onFinish, enabled = false)
                }
            }
        },
        centered = true,
    ) {
        // A failure replaces the parts with what the installer said.
        FadeThrough(install, { it is InstallState.Failed }, label = "install") { install ->
            if (install is InstallState.Failed) {
                install.detail?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
                    )
                }
            } else {
                InstallParts(install, lists)
            }
        }
    }
}

/** The buttons the install step shows. Finish turning on keeps its place; Try again fades through. */
private enum class InstallActions { Finish, TryAgain }

private fun installActions(install: InstallState): InstallActions = when {
    install is InstallState.Failed -> InstallActions.TryAgain
    else -> InstallActions.Finish
}

@Composable
private fun InstallParts(install: InstallState, lists: Int) {
    val stage = (install as? InstallState.Running)?.stage
    val steps = listOf(
        Progress(stringResource(R.string.feature_onboarding_step_module), stepState(install, InstallStage.Module, stage)),
        Progress(stringResource(R.string.feature_onboarding_step_apps), stepState(install, InstallStage.Apps, stage)),
        Progress(
            pluralStringResource(R.plurals.feature_onboarding_step_lists, lists, lists),
            if ((install as? InstallState.Done)?.listsFailed == true) StepState.Skipped else stepState(install, InstallStage.Lists, stage),
        ),
        Progress(stringResource(R.string.feature_onboarding_step_start), if (install is InstallState.Done) StepState.Done else StepState.Waiting),
    )
    Column {
        ProgressRows(steps)
        RevealNotNull(install as? InstallState.Running) { InstallProgress(it.progress) }
    }
}

/** A part of setting up: what it is, where it is, and, before it starts, what it involves. */
internal class Progress(val label: String, val state: StepState, val supporting: String? = null, val icon: ImageVector? = null)

@Composable
internal fun ProgressRows(rows: List<Progress>) {
    SegmentedGroup(rows.size, Modifier.padding(top = 12.dp)) { index ->
        val row = rows[index]
        val supporting = row.supporting
        SegmentedItem(
            index = index,
            count = rows.size,
            headline = row.label,
            supporting = if (supporting != null) {
                { Text(supporting) }
            } else {
                null
            },
            leading = {
                // A part done, running or skipped fades through to its new mark.
                Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                    FadeThrough(row.state, { it }, contentAlignment = Alignment.Center, label = "part") { state ->
                        when {
                            state == StepState.Done -> Icon(AdbIcons.CheckCircleFilled, contentDescription = null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
                            state == StepState.Running -> CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.5.dp)
                            state == StepState.Skipped -> Icon(AdbIcons.Info, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            row.icon != null -> Icon(row.icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            else -> Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.outlineVariant))
                        }
                    }
                }
            },
        )
    }
}

/** Longer than five seconds: a determinate progress indicator (M3). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun InstallProgress(progress: Float) {
    val label = stringResource(R.string.feature_onboarding_install_progress)
    LinearWavyProgressIndicator(
        progress = { progress },
        modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp).padding(top = 16.dp).semantics { contentDescription = label },
    )
}

/** A part of setting up against where it is: those before the running one are done. */
private fun stepState(install: InstallState, part: InstallStage, running: InstallStage?): StepState = when (install) {
    is InstallState.Done -> StepState.Done

    is InstallState.Running -> when {
        running == null -> StepState.Waiting
        part.ordinal < running.ordinal -> StepState.Done
        part == running -> StepState.Running
        else -> StepState.Waiting
    }

    else -> StepState.Waiting
}
