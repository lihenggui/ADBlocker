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

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SmallExtendedFloatingActionButton
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.Pop
import li.mercury.adblocker.core.ui.Reveal
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.failureText
import li.mercury.adblocker.core.ui.formatClockTime
import li.mercury.adblocker.core.ui.pageMargin
import li.mercury.adblocker.core.ui.profileAppLabel
import li.mercury.adblocker.core.ui.segmentedColors

@Composable
fun ActivityScreen(
    selectedId: Long?,
    onOpenRequest: (RequestEvent) -> Unit,
    onOpenSettings: () -> Unit,
    viewModel: ActivityViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle(minActiveState = Lifecycle.State.RESUMED)
    val context = LocalContext.current
    val startRecording = { viewModel.startRecording { ActivityRecordingService.start(context) } }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        // Recording is still available when Android hides its notification.
        startRecording()
    }
    // Apps chosen and filtering paused or resumed elsewhere meanwhile.
    LifecycleStartEffect(viewModel) {
        viewModel.refresh()
        onStopOrDispose {}
    }
    ActivityScreen(
        state = state,
        selectedId = selectedId,
        onVerdict = viewModel::setVerdict,
        onApp = viewModel::setApp,
        onStartRecording = {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            } else {
                startRecording()
            }
        },
        onStopRecording = { ActivityRecordingService.stop(context) },
        onOpenRequest = onOpenRequest,
        onOpenSettings = onOpenSettings,
        modifier = modifier,
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ActivityScreen(
    state: ActivityUiState,
    selectedId: Long?,
    onVerdict: (VerdictFilter) -> Unit,
    onApp: (String?) -> Unit,
    onStartRecording: () -> Unit,
    onStopRecording: () -> Unit,
    onOpenRequest: (RequestEvent) -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    val margin = pageMargin(LocalCompactWindow.current)
    Scaffold(
        modifier = modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_activity_title)) },
                actions = {
                    IconButton(onClick = onOpenSettings) { Icon(AdbIcons.Settings, contentDescription = stringResource(R.string.feature_activity_settings)) }
                },
                scrollBehavior = scrollBehavior,
            )
        },
        floatingActionButton = {
            // Away while recording starts, back once it runs.
            Pop(state.recording != RecordingStatus.Starting) {
                val recording = state.recording == RecordingStatus.Recording
                val label = stringResource(if (recording) R.string.feature_activity_stop_recording else R.string.feature_activity_start_recording)
                SmallExtendedFloatingActionButton(
                    text = { Text(label) },
                    icon = { Icon(if (recording) AdbIcons.Stop else AdbIcons.FiberManualRecord, contentDescription = null) },
                    onClick = if (recording) onStopRecording else onStartRecording,
                    modifier = Modifier.semantics {
                        contentDescription = label
                        role = Role.Button
                    },
                )
            }
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth)) {
            Reveal(state.recording != RecordingStatus.Idle) {
                Column {
                    Filters(state, onVerdict, onApp, Modifier.padding(horizontal = margin))
                    RecordingLine(state, Modifier.padding(horizontal = margin + 16.dp, vertical = 8.dp))
                }
            }
            RecordedRequests(state, selectedId, margin, onOpenRequest)
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun RecordedRequests(
    state: ActivityUiState,
    selectedId: Long?,
    margin: Dp,
    onOpenRequest: (RequestEvent) -> Unit,
) {
    // Keyed on what the empty page says, or on there being rows: new requests move the rows, not the page.
    FadeThrough(state, { if (it.shown.isEmpty()) emptyTitle(it) else null }, Modifier.fillMaxSize(), label = "requests") { state ->
        val shown = state.shown
        when {
            shown.isEmpty() -> Placeholder(
                AdbIcons.ReceiptLong,
                stringResource(emptyTitle(state)),
                text = if (state.recording == RecordingStatus.Idle) stringResource(R.string.feature_activity_recording_intro) else null,
            )

            else -> LazyColumn(contentPadding = PaddingValues(start = margin, end = margin, bottom = 96.dp), modifier = Modifier.fillMaxSize()) {
                itemsIndexed(shown, key = { _, it -> it.id }, contentType = { _, _ -> "request" }) { index, event ->
                    // New requests come in at the top; the others make room.
                    RequestRow(
                        event = event,
                        app = state.apps.valueOrNull?.byKey?.get(event.appKey),
                        position = RequestRowPosition.of(index, shown.size),
                        selected = event.id == selectedId,
                        onClick = { onOpenRequest(event) },
                        modifier = Modifier.animatedRow(this, MaterialTheme.motionScheme),
                    )
                }
            }
        }
    }
}

private fun emptyTitle(state: ActivityUiState): Int = when {
    state.recording == RecordingStatus.Idle -> R.string.feature_activity_no_recording
    state.events.isNotEmpty() -> R.string.feature_activity_no_matches
    state.recording == RecordingStatus.Stopped -> R.string.feature_activity_empty_result
    state.idle.valueOrNull == true -> R.string.feature_activity_empty_idle
    else -> R.string.feature_activity_empty
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Filters(state: ActivityUiState, onVerdict: (VerdictFilter) -> Unit, onApp: (String?) -> Unit, modifier: Modifier = Modifier) {
    var appMenu by remember { mutableStateOf(false) }
    Row(modifier.horizontalScroll(rememberScrollState()).selectableGroup(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        VerdictFilter.entries.forEach { filter ->
            val selected = state.verdict == filter
            FilterChip(
                selected = selected,
                onClick = { onVerdict(filter) },
                label = {
                    Text(
                        stringResource(
                            when (filter) {
                                VerdictFilter.All -> R.string.feature_activity_all
                                VerdictFilter.Blocked -> R.string.feature_activity_blocked
                                VerdictFilter.Allowed -> R.string.feature_activity_allowed
                                VerdictFilter.Encrypted -> R.string.feature_activity_encrypted
                            },
                        ),
                    )
                },
                leadingIcon = if (selected) {
                    { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(FilterChipDefaults.IconSize)) }
                } else {
                    null
                },
                border = FilterChipDefaults.filterChipBorder(enabled = true, selected = selected, borderColor = MaterialTheme.colorScheme.outline),
                modifier = Modifier.semantics { role = Role.RadioButton },
            )
        }
        // The whole chip opens the menu (M3 chips, compact windows).
        Box {
            val app = state.packageName?.let { state.apps.valueOrNull?.byKey?.get(it) }
            val identity = state.packageName?.let(AppIdentity::fromKey)
            FilterChip(
                selected = state.packageName != null,
                onClick = { appMenu = true },
                label = {
                    Text(
                        if (identity == null) {
                            stringResource(R.string.feature_activity_all_apps)
                        } else {
                            profileAppLabel(app?.label ?: identity.packageName.removePrefix("@"), identity.packageName, identity.profileSerial != null, identity.isSharedGroup)
                        },
                    )
                },
                leadingIcon = { Icon(AdbIcons.Apps, contentDescription = null, Modifier.size(FilterChipDefaults.IconSize)) },
                trailingIcon = { Icon(AdbIcons.ArrowDropDown, contentDescription = null, Modifier.size(FilterChipDefaults.IconSize)) },
                border = FilterChipDefaults.filterChipBorder(enabled = true, selected = state.packageName != null, borderColor = MaterialTheme.colorScheme.outline),
            )
            DropdownMenuPopup(expanded = appMenu, onDismissRequest = { appMenu = false }) {
                val options: List<InstalledApp?> = listOf(null) + state.apps.valueOrNull?.filtered.orEmpty()
                DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
                    options.forEachIndexed { index, option ->
                        DropdownMenuItem(
                            onClick = {
                                appMenu = false
                                onApp(option?.appKey)
                            },
                            text = {
                                Text(
                                    if (option == null) {
                                        stringResource(R.string.feature_activity_all_apps)
                                    } else {
                                        profileAppLabel(option.label, option.packageName, option.profileSerial != null, option.isSharedGroup)
                                    },
                                )
                            },
                            shape = MenuDefaults.itemShape(index, options.size).shape,
                            leadingIcon = option?.let { { AppIcon(it, it.label, size = 24.dp) } },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun RecordingLine(state: ActivityUiState, modifier: Modifier = Modifier) {
    val live = state.recording == RecordingStatus.Recording
    val status = stringResource(
        when {
            state.failure != null -> R.string.feature_activity_interrupted
            state.recording == RecordingStatus.Starting -> R.string.feature_activity_starting
            live -> R.string.feature_activity_recording
            else -> R.string.feature_activity_recorded
        },
    )
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        LiveDot(live)
        Column {
            Text(
                status + if (state.dropped > 0) " " + stringResource(R.string.feature_activity_dropped, state.dropped) else "",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            RevealNotNull(state.failure) {
                Text(failureText(it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            Reveal(state.apps is ReadState.Unavailable) {
                Text(stringResource(R.string.feature_activity_apps_unavailable), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Reveal((state.recording == RecordingStatus.Starting || live) && state.idle is ReadState.Unavailable) {
                Text(stringResource(R.string.feature_activity_status_unavailable), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** A dot that breathes while requests come in, and rests in the outline color when nothing can. */
@Composable
private fun LiveDot(live: Boolean) {
    // A repeating breath has no spring: it keeps its duration and easing. It
    // runs only while live, and is read while drawing, so that it does not
    // compose the line again on every frame.
    val pulse = if (live) {
        rememberInfiniteTransition(label = "live").animateFloat(
            initialValue = 1f,
            targetValue = PULSE_LOW,
            animationSpec = infiniteRepeatable(tween(PULSE_MS, easing = FastOutSlowInEasing), RepeatMode.Reverse),
            label = "live alpha",
        )
    } else {
        null
    }
    val color by animateColorAsState(
        if (live) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "live color",
    )
    Box(Modifier.size(8.dp).graphicsLayer { alpha = pulse?.value ?: 1f }.clip(CircleShape).background(color))
}

private const val PULSE_MS = 1_000
private const val PULSE_LOW = 0.25f

internal enum class RequestRowPosition(val index: Int, val count: Int) {
    Only(0, 1),
    First(0, 2),
    Middle(1, 3),
    Last(1, 2),
    ;

    companion object {
        fun of(index: Int, count: Int): RequestRowPosition = when {
            count == 1 -> Only
            index == 0 -> First
            index == count - 1 -> Last
            else -> Middle
        }
    }
}

/** One request: the app and host first, then what became of it, in words and an icon as well as color. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun RequestRow(
    event: RequestEvent,
    app: InstalledApp?,
    position: RequestRowPosition,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val (icon, label, color) = verdictLook(event.verdict)
    Box(modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            selected = selected && LocalShowsDetailBesideList.current,
            onClick = onClick,
            shapes = ListItemDefaults.segmentedShapes(position.index, position.count),
            colors = segmentedColors(),
            leadingContent = { AppIcon(app, app?.label ?: event.packageName) },
            supportingContent = {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(icon, contentDescription = null, Modifier.size(16.dp), tint = color)
                    Text(stringResource(label), color = color, style = MaterialTheme.typography.bodyMedium, maxLines = 1)
                    // The app and the kind give way first: the verdict is what the row is for.
                    Text(
                        "· " + profileAppLabel(app?.label ?: event.packageName.removePrefix("@"), event.packageName, event.profileSerial != null, event.packageName.startsWith('@')) + (event.kind?.let { " · " + kindName(it) } ?: ""),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
            },
            trailingContent = { Text(formatClockTime(LocalContext.current, event.time, includeSeconds = true), style = MaterialTheme.typography.labelMedium.copy(fontFeatureSettings = "tnum")) },
            content = {
                Text(event.host, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace))
            },
        )
    }
}

@Composable
internal fun verdictLook(verdict: Verdict) = when (verdict) {
    Verdict.Blocked -> Triple(AdbIcons.Block, R.string.feature_activity_verdict_blocked, MaterialTheme.colorScheme.primary)
    Verdict.HostBlocked -> Triple(AdbIcons.Block, R.string.feature_activity_verdict_host_blocked, MaterialTheme.colorScheme.primary)
    Verdict.Allowed -> Triple(AdbIcons.Check, R.string.feature_activity_verdict_allowed, MaterialTheme.colorScheme.onSurfaceVariant)
    Verdict.Passed -> Triple(AdbIcons.Lock, R.string.feature_activity_verdict_passed, MaterialTheme.colorScheme.tertiary)
}

@Composable
internal fun kindName(kind: String) = when (kind) {
    "script" -> stringResource(R.string.feature_activity_kind_script)
    "image" -> stringResource(R.string.feature_activity_kind_image)
    "document", "subdocument" -> stringResource(R.string.feature_activity_kind_document)
    "stylesheet" -> stringResource(R.string.feature_activity_kind_stylesheet)
    "font" -> stringResource(R.string.feature_activity_kind_font)
    "media" -> stringResource(R.string.feature_activity_kind_media)
    "xmlhttprequest" -> "XHR"
    else -> kind
}
