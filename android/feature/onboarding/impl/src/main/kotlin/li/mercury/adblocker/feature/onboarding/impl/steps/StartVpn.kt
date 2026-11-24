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

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.data.repository.VpnTransferPart
import li.mercury.adblocker.core.designsystem.component.MarkDefaults
import li.mercury.adblocker.core.designsystem.component.MarkState
import li.mercury.adblocker.core.designsystem.component.StatusMark
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.failureText
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.feature.onboarding.impl.InstallStage
import li.mercury.adblocker.feature.onboarding.impl.InstallState
import li.mercury.adblocker.feature.onboarding.impl.OnboardingUiState
import li.mercury.adblocker.feature.onboarding.impl.R

/**
 * The VPN's start: the two grants the system asks for, then the start and
 * the download. A refused consent comes back here, saying so; the barrier
 * drops once the VPN runs.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.StartVpn(state: OnboardingUiState, onStart: () -> Unit, onFinish: () -> Unit) {
    val install = state.install
    val lists = state.lists.count { it.value }
    val done = install as? InstallState.Done
    val hasApps = state.picks.isNotEmpty()
    StepPage(
        title = when (install) {
            is InstallState.Done -> if (install.active) {
                pluralStringResource(R.plurals.feature_onboarding_done_title, install.apps, install.apps)
            } else {
                stringResource(R.string.feature_onboarding_filter_later_title)
            }

            is InstallState.Failed -> stringResource(R.string.feature_onboarding_start_failed_title)

            is InstallState.Running -> stringResource(R.string.feature_onboarding_starting_title)

            InstallState.Idle -> stringResource(if (hasApps) R.string.feature_onboarding_step_start else R.string.feature_onboarding_filter_later_title)
        },
        text = when (install) {
            is InstallState.Done -> stringResource(
                when {
                    !install.active -> R.string.feature_onboarding_filter_later_text
                    state.notifications == true -> R.string.feature_onboarding_done_text_vpn
                    else -> R.string.feature_onboarding_done_text
                },
            )

            is InstallState.Failed -> if (install.failure == LoadFailure.Failed) stringResource(R.string.feature_onboarding_start_failed_text) else failureText(install.failure)

            is InstallState.Running -> null

            InstallState.Idle -> stringResource(if (hasApps) R.string.feature_onboarding_start_text else R.string.feature_onboarding_filter_later_text)
        },
        art = { StatusMark(MarkState.Filtering, size = 200.dp, colors = MarkDefaults.featuredColors(), armDown = done?.active == true) },
        action = {
            FadeThrough(install, ::vpnActions, label = "start action") { install ->
                when (install) {
                    is InstallState.Done -> StepAction(stringResource(R.string.feature_onboarding_finish), onFinish)
                    is InstallState.Failed -> StepAction(stringResource(R.string.feature_onboarding_try_again), onStart)
                    is InstallState.Running -> StepAction(stringResource(R.string.feature_onboarding_finish), onFinish, enabled = false)
                    InstallState.Idle -> StepAction(stringResource(if (hasApps) R.string.feature_onboarding_step_start else R.string.feature_onboarding_save_setup), onStart)
                }
            }
        },
        centered = true,
    ) {
        RevealNotNull(state.vpnTransferFailure) { failure ->
            Column {
                if (failure.failed.isEmpty()) Hint(stringResource(R.string.feature_onboarding_vpn_transfer_incomplete), centered = true)
                for (part in failure.failed.sorted()) Hint(stringResource(part.failureText()), centered = true)
            }
        }
        // A failure takes the parts away; the words above say what went wrong.
        Reveal(hasApps && install !is InstallState.Failed) { VpnParts(state, install, done, lists) }
    }
}

private fun VpnTransferPart.failureText() = when (this) {
    VpnTransferPart.Apps -> R.string.feature_onboarding_vpn_transfer_apps
    VpnTransferPart.Lists -> R.string.feature_onboarding_vpn_transfer_lists
    VpnTransferPart.Rules -> R.string.feature_onboarding_vpn_transfer_rules
    VpnTransferPart.TrustedSites -> R.string.feature_onboarding_vpn_transfer_trusted_sites
    VpnTransferPart.UrlFiltering -> R.string.feature_onboarding_vpn_transfer_url_filtering
}

/** The button the VPN step shows. Finish turning on keeps its place; other buttons fade through. */
private enum class VpnActions { Start, Finish, TryAgain }

private fun vpnActions(install: InstallState): VpnActions = when (install) {
    InstallState.Idle -> VpnActions.Start
    is InstallState.Failed -> VpnActions.TryAgain
    is InstallState.Running, is InstallState.Done -> VpnActions.Finish
}

@Composable
private fun VpnParts(state: OnboardingUiState, install: InstallState, done: InstallState.Done?, lists: Int) {
    Column {
        ProgressRows(vpnProgress(state, install, done, lists))
        RevealNotNull(install as? InstallState.Running) { InstallProgress(it.progress) }
        Reveal(state.consentRefused && install == InstallState.Idle) {
            Text(
                stringResource(R.string.feature_onboarding_vpn_refused),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.error,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp),
            )
        }
        Reveal(done != null) { Hint(stringResource(R.string.feature_onboarding_done_https), centered = true) }
    }
}

@Composable
private fun vpnProgress(state: OnboardingUiState, install: InstallState, done: InstallState.Done?, lists: Int): List<Progress> {
    val stage = (install as? InstallState.Running)?.stage
    val vpnState = when {
        done != null -> StepState.Done
        stage == InstallStage.Apps || stage == InstallStage.Vpn -> StepState.Running
        stage == InstallStage.Lists -> StepState.Done
        else -> StepState.Waiting
    }
    val listsState = when {
        done != null -> if (done.listsFailed) StepState.Skipped else StepState.Done
        stage == InstallStage.Lists -> StepState.Running
        else -> StepState.Waiting
    }
    val notificationsState = when (state.notifications) {
        true -> StepState.Done
        false -> StepState.Skipped
        null -> StepState.Waiting
    }
    return if (done == null) {
        listOf(
            Progress(stringResource(R.string.feature_onboarding_grant_notifications), notificationsState, stringResource(R.string.feature_onboarding_grant_notifications_text), AdbIcons.Notifications),
            Progress(stringResource(R.string.feature_onboarding_grant_vpn), vpnState, stringResource(R.string.feature_onboarding_grant_vpn_text), AdbIcons.VpnKey),
            Progress(pluralStringResource(R.plurals.feature_onboarding_step_lists, lists, lists), listsState, stringResource(R.string.feature_onboarding_grant_lists_text), AdbIcons.Download),
        )
    } else {
        listOf(
            if (state.notifications == true) {
                Progress(stringResource(R.string.feature_onboarding_notifications_allowed), notificationsState)
            } else {
                Progress(stringResource(R.string.feature_onboarding_notifications_off), notificationsState, stringResource(R.string.feature_onboarding_notifications_off_text))
            },
            Progress(stringResource(R.string.feature_onboarding_vpn_connected), vpnState, stringResource(R.string.feature_onboarding_vpn_connected_text)),
            if (done.listsFailed) {
                Progress(stringResource(R.string.feature_onboarding_lists_failed), listsState, stringResource(R.string.feature_onboarding_lists_failed_text))
            } else {
                val count = pluralStringResource(R.plurals.feature_onboarding_lists_count, lists, lists)
                Progress(
                    stringResource(R.string.feature_onboarding_lists_downloaded),
                    listsState,
                    done.rules?.let { stringResource(R.string.feature_onboarding_lists_summary, count, pluralStringResource(R.plurals.feature_onboarding_rules, it, formatCount(it))) } ?: count,
                )
            },
        )
    }
}
