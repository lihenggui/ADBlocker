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

package li.mercury.adblocker.feature.activity.impl

import android.content.ClipData
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.PassReason
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.formatClockTime
import li.mercury.adblocker.core.ui.profileAppLabel

/**
 * One request: in a modal bottom sheet over the list on phones, in the
 * detail pane beside it on wide screens. What decided, and what to do about
 * it, where the problem is seen.
 */
@Composable
fun RequestScreen(onClose: () -> Unit, viewModel: RequestViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    // Done in the sheet: close it. Beside the list, the pane stays.
    val beside = LocalShowsDetailBesideList.current
    LaunchedEffect(state.done, beside) { if (state.done && !beside) onClose() }
    // The request is kept in memory only: after the process ended it is gone.
    LaunchedEffect(state.gone) { if (state.gone) onClose() }
    val request = state.request ?: return
    Box(modifier) {
        RequestContent(
            request = request,
            state = state,
            onTrust = viewModel::trustSite,
            onBlock = viewModel::blockHost,
            onBlockInApp = viewModel::blockHostInApp,
            onForget = viewModel::forgetPinned,
        )
        // Shown here only as a pane; in a sheet, the screen below shows it.
        AppSnackbarHost(Modifier.align(Alignment.BottomCenter))
    }
}

@Composable
internal fun RequestContent(
    request: RequestEvent,
    state: RequestUiState,
    onTrust: () -> Unit,
    onBlock: () -> Unit,
    onBlockInApp: () -> Unit,
    onForget: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(top = 8.dp, bottom = 24.dp).navigationBarsPadding()) {
        RequestVerdict(request)
        SelectionContainer {
            Column {
                Text(request.host, style = MaterialTheme.typography.headlineSmall.copy(fontFamily = FontFamily.Monospace), modifier = Modifier.padding(top = 16.dp))
                request.url?.let {
                    Text(it, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        Spacer(Modifier.padding(top = 12.dp))
        RequestFacts(request, state)
        RequestActions(
            verdict = request.verdict,
            reason = request.reason,
            trustSite = state.trustSite,
            blockApp = state.blockPackage?.let { state.app?.label ?: it },
            onTrust = onTrust,
            onBlock = onBlock,
            onBlockInApp = onBlockInApp,
            onForget = onForget,
            onCopy = {
                scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(request.host, request.url ?: request.host))) }
            },
        )
    }
}

@Composable
private fun RequestVerdict(request: RequestEvent) {
    val (icon, label) = verdictLook(request.verdict)
    Row(verticalAlignment = Alignment.CenterVertically) {
        // The verdict on its own container: primary for a block, tertiary for
        // what was not decrypted, neutral for what was let through.
        val (pillContainer, pillContent) = when (request.verdict) {
            Verdict.Blocked, Verdict.HostBlocked -> MaterialTheme.colorScheme.primaryContainer to MaterialTheme.colorScheme.onPrimaryContainer
            Verdict.Passed -> MaterialTheme.colorScheme.tertiaryContainer to MaterialTheme.colorScheme.onTertiaryContainer
            Verdict.Allowed -> MaterialTheme.colorScheme.surfaceContainerHighest to MaterialTheme.colorScheme.onSurfaceVariant
        }
        Surface(shape = MaterialTheme.shapes.small, color = pillContainer, contentColor = pillContent) {
            Row(Modifier.height(32.dp).padding(start = 8.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(icon, contentDescription = null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(label), style = MaterialTheme.typography.labelLarge)
            }
        }
        Spacer(Modifier.weight(1f))
        Text(
            formatClockTime(LocalContext.current, request.time, includeSeconds = true),
            style = MaterialTheme.typography.labelMedium.copy(fontFamily = FontFamily.Monospace, fontFeatureSettings = "tnum"),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun RequestFacts(request: RequestEvent, state: RequestUiState) {
    val reason = request.reason
    Fact(stringResource(R.string.feature_activity_app)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val label = state.app?.label ?: request.packageName.removePrefix("@")
            AppIcon(state.app, label, size = 24.dp)
            Text(profileAppLabel(label, request.profileSerial != null, request.packageName.startsWith('@')))
        }
    }
    request.kind?.let { Fact(stringResource(R.string.feature_activity_kind)) { Text(kindName(it)) } }
    request.page?.let { Fact(stringResource(R.string.feature_activity_page)) { Text(it, fontFamily = FontFamily.Monospace) } }
    if (state.ruleCheckUnavailable) {
        Fact(stringResource(R.string.feature_activity_rule)) { Text(stringResource(R.string.feature_activity_rule_unavailable)) }
    }
    state.rule?.rule?.let { rule ->
        // The rule as code, then the list it is in, by its name.
        Fact(stringResource(R.string.feature_activity_rule)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                Text(
                    rule,
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    modifier = Modifier
                        .background(MaterialTheme.colorScheme.surfaceContainerHighest, RoundedCornerShape(4.dp))
                        .padding(horizontal = 6.dp, vertical = 1.dp),
                )
                state.rule.source?.let {
                    Text(
                        when (it) {
                            RuleSource.UserRules -> stringResource(R.string.feature_activity_my_rules)
                            is RuleSource.List -> FilterCatalog.byUrl(it.url)?.title ?: it.url.substringAfter("://").substringBefore('/')
                        },
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
    // Why a connection was refused whole, or handed on unread.
    when {
        request.verdict == Verdict.HostBlocked -> Fact(stringResource(R.string.feature_activity_why)) { Text(stringResource(R.string.feature_activity_reason_host_blocked)) }
        reason != null -> Fact(stringResource(R.string.feature_activity_why)) { Text(stringResource(reasonText(reason))) }
    }
}

@Composable
private fun RequestActions(
    verdict: Verdict,
    reason: PassReason?,
    trustSite: String?,
    /** The app's label when the host can be blocked in that app alone. */
    blockApp: String?,
    onTrust: () -> Unit,
    onBlock: () -> Unit,
    onBlockInApp: () -> Unit,
    onForget: () -> Unit,
    onCopy: () -> Unit,
) {
    FlowRow(Modifier.padding(top = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        when {
            trustSite != null && (verdict == Verdict.Blocked || verdict == Verdict.HostBlocked) -> Button(onClick = onTrust) {
                Icon(AdbIcons.VerifiedUser, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
                Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                Text(stringResource(R.string.feature_activity_trust_site, trustSite))
            }

            // The request's own app first; every app is the wider alternative.
            verdict == Verdict.Allowed && blockApp != null -> {
                BlockButton(stringResource(R.string.feature_activity_block_in_app, blockApp), onBlockInApp)
                FilledTonalButton(onClick = onBlock) { Text(stringResource(R.string.feature_activity_block_in_all_apps)) }
            }

            verdict == Verdict.Allowed -> BlockButton(stringResource(R.string.feature_activity_block_host), onBlock)

            reason == PassReason.Pinned -> FilledTonalButton(onClick = onForget) { Text(stringResource(R.string.feature_activity_forget)) }
        }
        OutlinedButton(onClick = onCopy) {
            Icon(AdbIcons.ContentCopy, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
            Spacer(Modifier.size(ButtonDefaults.IconSpacing))
            Text(stringResource(R.string.feature_activity_copy))
        }
    }
}

@Composable
private fun BlockButton(label: String, onClick: () -> Unit) {
    Button(onClick = onClick) {
        Icon(AdbIcons.Block, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
        Text(label)
    }
}

@Composable
private fun Fact(label: String, value: @Composable () -> Unit) {
    Column {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp)) {
            Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(96.dp))
            Column(Modifier.weight(1f)) {
                CompositionLocalProvider(LocalTextStyle provides MaterialTheme.typography.bodyMedium) { value() }
            }
        }
    }
}

private fun reasonText(reason: PassReason) = when (reason) {
    PassReason.Pinned -> R.string.feature_activity_reason_pinned
    PassReason.ExclusionsPending -> R.string.feature_activity_reason_exclusions_pending
    PassReason.DomainOnly -> R.string.feature_activity_reason_hosts_only
    PassReason.Excluded -> R.string.feature_activity_reason_excluded
    PassReason.Trusted -> R.string.feature_activity_reason_trusted
    PassReason.Sensitive -> R.string.feature_activity_reason_sensitive
    PassReason.NoName -> R.string.feature_activity_reason_no_name
    PassReason.Untrusted -> R.string.feature_activity_reason_untrusted
    PassReason.Unverified -> R.string.feature_activity_reason_unverified
    PassReason.Other -> R.string.feature_activity_reason_other
}
