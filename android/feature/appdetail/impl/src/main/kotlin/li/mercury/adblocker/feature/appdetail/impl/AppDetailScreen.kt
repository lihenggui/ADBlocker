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

package li.mercury.adblocker.feature.appdetail.impl

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.pm.LauncherApps
import android.os.UserManager
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.ContentKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FailureView
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.LocalShowsDetailBesideList
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.Pop
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.appLabelArgument
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.core.ui.message
import li.mercury.adblocker.core.ui.pageMargin
import li.mercury.adblocker.core.ui.profileAppLabel
import li.mercury.adblocker.core.ui.R as UiR

@Composable
fun AppDetailScreen(
    onBack: () -> Unit,
    onOpenActivity: () -> Unit,
    onOpenHttps: () -> Unit,
    onOpenWorkHttps: () -> Unit,
    viewModel: AppDetailViewModel,
    modifier: Modifier = Modifier,
) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    LifecycleResumeEffect(viewModel) {
        viewModel.refresh()
        onPauseOrDispose {}
    }
    AppDetailScreen(
        state = state,
        onBack = onBack,
        onFilterChange = viewModel::setFiltered,
        onPause = viewModel::pause,
        onResume = viewModel::resume,
        onTogglePins = viewModel::togglePinned,
        onForget = viewModel::forget,
        onOpenActivity = {
            viewModel.showActivity()
            onOpenActivity()
        },
        onRetry = viewModel::refresh,
        modifier = modifier,
        onOpenHttps = onOpenHttps,
        onOpenWorkHttps = onOpenWorkHttps,
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun AppDetailScreen(
    state: AppDetailUiState,
    onBack: () -> Unit,
    onFilterChange: (Boolean) -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onTogglePins: () -> Unit,
    onForget: (String?) -> Unit,
    onOpenActivity: () -> Unit,
    onRetry: () -> Unit,
    modifier: Modifier = Modifier,
    onOpenHttps: () -> Unit = {},
    onOpenWorkHttps: () -> Unit = {},
) {
    val context = LocalContext.current
    val scrollBehavior = TopAppBarDefaults.pinnedScrollBehavior()
    // Beside the list, the page needs no way back (M3 list-detail).
    val beside = LocalShowsDetailBesideList.current
    Scaffold(
        modifier = modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            TopAppBar(
                title = {},
                navigationIcon = {
                    if (!beside) {
                        IconButton(onClick = onBack) { Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_appdetail_back)) }
                    }
                },
                actions = {
                    val launchable = state.entry?.takeIf { entry ->
                        val work = entry.app.work
                        !entry.app.isSharedGroup && (work == null || (work.available && work.activity != null))
                    }
                    // Known once the app is read: the action arrives with it.
                    Pop(launchable != null) {
                        launchable?.let { entry ->
                            IconButton(onClick = { launch(context, entry.app) }) {
                                Icon(AdbIcons.OpenInNew, contentDescription = stringResource(R.string.feature_appdetail_open, appLabelArgument(entry.app.label, entry.app.packageName)))
                            }
                        }
                    }
                },
                scrollBehavior = scrollBehavior,
            )
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        val kind = { it: AppDetailUiState -> ContentKind.of(it.loading, it.failure, empty = it.entry == null) }
        FadeThrough(state, kind, Modifier.padding(padding).fillMaxSize(), Alignment.Center, "appDetail") { state ->
            when {
                state.loading -> LoadingIndicator()
                state.failure != null -> FailureView(state.failure, onRetry)
                state.entry == null -> Placeholder(AdbIcons.Apps, stringResource(R.string.feature_appdetail_not_installed))
                else -> Details(state, state.entry, onFilterChange, onPause, onResume, onTogglePins, onForget, onOpenActivity, onOpenHttps, onOpenWorkHttps)
            }
        }
    }
}

@Composable
private fun Details(
    state: AppDetailUiState,
    entry: AppEntry,
    onFilterChange: (Boolean) -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onTogglePins: () -> Unit,
    onForget: (String?) -> Unit,
    onOpenActivity: () -> Unit,
    onOpenHttps: () -> Unit,
    onOpenWorkHttps: () -> Unit,
) {
    val margin = pageMargin(LocalCompactWindow.current)
    // In a pane of its own (opened from the overview on a wide window) the page stops growing.
    Column(Modifier.fillMaxSize().wrapContentWidth().widthIn(max = MaxContentWidth).verticalScroll(rememberScrollState()).padding(horizontal = margin)) {
        Row(Modifier.padding(start = 4.dp, bottom = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            AppIcon(entry.app, entry.app.label, size = 56.dp)
            Column {
                Text(profileAppLabel(entry.app.label, entry.app.packageName, entry.app.profileSerial != null, entry.app.isSharedGroup), style = MaterialTheme.typography.headlineSmall)
                Text(
                    entry.packageName,
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (entry.app.sharedMembers.isNotEmpty()) {
            SectionHeader(stringResource(R.string.feature_appdetail_shared_members))
            Text(
                entry.app.sharedMembers.joinToString("\n") { member ->
                    if (member.label == member.packageName) member.packageName else "${member.label} · ${member.packageName}"
                },
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 12.dp),
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            )
        }
        val unavailable = entry.unavailableReason
        if (unavailable != null) {
            Note(
                AdbIcons.Info,
                if (unavailable == AppUnavailableReason.SharedUid) R.string.feature_appdetail_shared_uid else unavailable.message(),
                action = UiR.string.core_ui_remove_app_selection.takeIf { entry.isSelected },
                onAction = { onFilterChange(false) },
            )
        } else {
            MainSwitch(entry, onFilterChange)
            val workNote = when {
                entry.app.work?.quiet == true -> R.string.feature_appdetail_work_quiet
                entry.app.work?.available == false -> R.string.feature_appdetail_work_unavailable
                else -> null
            }
            RevealNotNull(workNote) { Note(AdbIcons.Info, it) }
            // Turning filtering on or off brings what limits it, or takes it away.
            RevealNotNull(levelNote(state, entry, onOpenHttps, onOpenWorkHttps)) { Note(it.icon, it.text, it.action, it.onAction) }
        }
        SectionHeader(stringResource(R.string.feature_appdetail_blocked))
        // The counts are read after the page: they fade in where the note was.
        FadeThrough(state.counts, { it::class }, label = "counts") { counts ->
            when (counts) {
                is ReadState.Available -> Counts(counts.value)
                is ReadState.Unavailable -> Note(AdbIcons.Info, R.string.feature_appdetail_counts_unavailable)
                ReadState.Pending -> Note(AdbIcons.Info, R.string.feature_appdetail_counts_checking)
            }
        }
        SectionHeader(stringResource(R.string.feature_appdetail_trouble))
        Trouble(state, entry, onPause, onResume, onTogglePins, onForget, onOpenActivity)
        Spacer(Modifier.height(24.dp))
    }
}

/**
 * The page's main switch: the whole row toggles it, as one target. Its
 * corners tighten while it is pressed, as M3 Expressive containers do.
 */
@Composable
private fun MainSwitch(entry: AppEntry, onFilterChange: (Boolean) -> Unit) {
    val motion = MaterialTheme.motionScheme
    val on = entry.isFiltered
    val container by animateColorAsState(
        if (on) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
        motion.defaultEffectsSpec(),
        label = "main switch",
    )
    val interactionSource = remember { MutableInteractionSource() }
    val pressed by interactionSource.collectIsPressedAsState()
    val corner by animateDpAsState(if (pressed) 16.dp else 28.dp, motion.fastSpatialSpec(), label = "main switch corner")
    Surface(
        shape = RoundedCornerShape(corner),
        color = container,
        contentColor = if (on) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            Modifier
                .toggleable(
                    value = on,
                    interactionSource = interactionSource,
                    indication = ripple(),
                    enabled = on || entry.app.work?.available != false,
                    role = Role.Switch,
                    onValueChange = onFilterChange,
                )
                .padding(start = 20.dp, end = 16.dp, top = 16.dp, bottom = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                val title = when {
                    entry.inAllAppsScope -> R.string.feature_appdetail_include
                    entry.app.isSharedGroup -> R.string.feature_appdetail_filter_group
                    else -> R.string.feature_appdetail_filter_this
                }
                Text(stringResource(title), style = MaterialTheme.typography.titleMediumEmphasized)
                Text(
                    stringResource(
                        when {
                            !on -> R.string.feature_appdetail_off

                            // What is done, which can be less than what was chosen.
                            entry.filter?.httpLevel == FilterLevel.Full && entry.filter?.level == FilterLevel.Hosts -> R.string.feature_appdetail_mixed

                            entry.filter?.level == FilterLevel.Hosts -> R.string.feature_appdetail_on_hosts

                            else -> R.string.feature_appdetail_on_full
                        },
                    ),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            Switch(
                checked = on,
                // The row handles the click; the switch only shows the state.
                onCheckedChange = null,
                thumbContent = if (on) {
                    { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
                } else {
                    null
                },
            )
        }
    }
}

/** Today's and the week's counts, and the week by day. A filled card. */
@Composable
private fun Counts(counts: AppCount?) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest), modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(24.dp)) {
            Count(stringResource(R.string.feature_appdetail_today), counts?.today?.total ?: 0)
            Count(stringResource(R.string.feature_appdetail_week), counts?.week?.total ?: 0)
            val days = counts?.days.orEmpty()
            val max = (days.maxOrNull() ?: 0L).coerceAtLeast(1)
            Row(Modifier.weight(1f).height(40.dp), horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.Bottom) {
                days.forEachIndexed { index, n ->
                    Box(
                        Modifier.weight(1f)
                            .fillMaxHeight((n.toFloat() / max).coerceAtLeast(0.08f))
                            .background(
                                // Outline: 3:1 on surfaces, as a graphic that carries data needs.
                                if (index == days.lastIndex) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
                                RoundedCornerShape(4.dp),
                            ),
                    )
                }
            }
        }
    }
}

@Composable
private fun Count(label: String, value: Long) {
    Column {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(formatCount(value), style = MaterialTheme.typography.headlineMediumEmphasized.copy(fontFeatureSettings = "tnum"))
    }
}

private fun launch(context: Context, app: InstalledApp) {
    try {
        val work = app.work
        if (work == null) {
            context.packageManager.getLaunchIntentForPackage(app.packageName)?.let(context::startActivity)
        } else {
            val users = context.getSystemService(UserManager::class.java) ?: return
            val activity = work.activity ?: return
            if (users.getSerialNumberForUser(work.user) != work.serial || users.isQuietModeEnabled(work.user)) return
            context.getSystemService(LauncherApps::class.java)?.startMainActivity(activity, work.user, null, null)
        }
    } catch (_: ActivityNotFoundException) {
        // The app changed since the detail page was read.
    } catch (_: SecurityException) {
        // Android may deny a launch after a profile or policy change.
    } catch (_: IllegalStateException) {
        // The profile can lock while a launch is pending.
    }
}

/** What a [Note] says, and the action it offers if there is one. */
private class NoteContent(val icon: ImageVector, val text: Int, val action: Int? = null, val onAction: () -> Unit = {})

/** Why the app is filtered less than chosen, if it is, and the setup that would change it. */
private fun levelNote(state: AppDetailUiState, entry: AppEntry, onOpenHttps: () -> Unit, onOpenWorkHttps: () -> Unit): NoteContent? {
    val filter = entry.filter ?: return null
    val reason = filter.levelReason
    val needsCertificate = state.method == FilterMethod.Vpn && (
        reason == LevelReason.CaNotInstalled || (reason == LevelReason.UrlFilteringOff && !state.certificateInstalled)
        )
    return when {
        reason == LevelReason.ProfileTrustUnavailable ->
            NoteContent(AdbIcons.Lock, R.string.feature_appdetail_work_certificate, R.string.feature_appdetail_set_up_https, onOpenWorkHttps)

        needsCertificate -> NoteContent(AdbIcons.Lock, R.string.feature_appdetail_level_needs_certificate, R.string.feature_appdetail_set_up_https, onOpenHttps)

        reason == LevelReason.UrlFilteringOff -> NoteContent(AdbIcons.Info, R.string.feature_appdetail_level_url_filtering_off)

        reason == LevelReason.ExclusionsPending -> NoteContent(AdbIcons.Info, R.string.feature_appdetail_exclusions_pending)

        reason == LevelReason.Unknown -> NoteContent(AdbIcons.Info, R.string.feature_appdetail_level_unknown)

        else -> null
    }
}

@Composable
private fun Note(icon: ImageVector, text: Int, action: Int? = null, onAction: () -> Unit = {}) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer, contentColor = MaterialTheme.colorScheme.onSecondaryContainer),
        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
    ) {
        Row(Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = if (action == null) 16.dp else 4.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(icon, contentDescription = null, Modifier.size(20.dp))
            Column(Modifier.weight(1f)) {
                Text(stringResource(text), style = MaterialTheme.typography.bodyMedium)
                if (action != null) {
                    TextButton(
                        onClick = onAction,
                        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onSecondaryContainer),
                        modifier = Modifier.align(Alignment.End),
                    ) { Text(stringResource(action)) }
                }
            }
        }
    }
}
