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

package li.mercury.adblocker.feature.overview.impl

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuAnchorPosition
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.SelectableDropdownMenuItem
import androidx.compose.material3.SplitButtonDefaults
import androidx.compose.material3.SplitButtonLayout
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.component.MarkState
import li.mercury.adblocker.core.designsystem.component.StatusMark
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.designsystem.motion.swapTransform
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.core.ui.rememberNow
import li.mercury.adblocker.core.ui.timeLeft
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

/** The pause durations offered, in minutes; null lasts until resumed. */
private val PAUSES: List<Int?> = listOf(15, 60, null)

/** The duration the split button's main action pauses for, marked in its menu. */
private const val DEFAULT_PAUSE = 60

/** What the status card's button can do. */
internal data class HeroActions(
    val pause: (Int?) -> Unit,
    val resume: () -> Unit,
    val start: () -> Unit,
    val turnOnAgain: () -> Unit,
    val reconnect: () -> Unit,
)

/**
 * What the status card says: its title, the line under it, and its one
 * action. The module's service can be stopped or turned off; the VPN can be
 * connecting, or disconnected.
 */
private enum class Hero { Filtering, Paused, Stopped, TurnedOff, Connecting, Disconnected }

/**
 * Whether filtering is on, and the one control to change it. The control is
 * a split button: its main action pauses for an hour, and its menu offers
 * other durations; a pause always ends by itself unless chosen otherwise.
 * When the state changes, the barrier moves, the shape morphs, and the words
 * and the button are swapped: the old fade, then the new rise into place.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun StatusHero(status: DaemonStatus, filteredApps: Int, actions: HeroActions, vpn: VpnStatus?) {
    val motion = MaterialTheme.motionScheme
    val hero = when {
        // Without root, filtering runs while the VPN does.
        vpn != null -> when (vpn.state) {
            VpnState.On -> if (status.paused) Hero.Paused else Hero.Filtering
            VpnState.Starting -> Hero.Connecting
            VpnState.Off, VpnState.Revoked, VpnState.Failed -> Hero.Disconnected
        }

        !status.running && status.bootGuard != null -> Hero.TurnedOff

        !status.running -> Hero.Stopped

        status.paused -> Hero.Paused

        else -> Hero.Filtering
    }
    val mark = when (hero) {
        Hero.Filtering -> MarkState.Filtering
        Hero.Paused, Hero.Connecting -> MarkState.Paused
        Hero.Stopped, Hero.TurnedOff, Hero.Disconnected -> MarkState.Stopped
    }
    val container by animateColorAsState(
        if (hero == Hero.Filtering) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer,
        motion.slowEffectsSpec(),
        label = "hero",
    )
    val content = if (hero == Hero.Filtering) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface
    val rise = with(LocalDensity.current) { SWAP_RISE.roundToPx() }
    Surface(
        // Extra large: a featured container, not an information-dense card.
        shape = RoundedCornerShape(28.dp),
        color = container,
        contentColor = content,
        modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
    ) {
        Column(
            Modifier.padding(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            StatusMark(mark)
            // While the words swap, the old and the new are both laid out: the
            // status is said once, from here, as the current one.
            val title = stringResource(heroTitle(hero))
            AnimatedContent(
                targetState = hero,
                transitionSpec = { swapTransform(motion, rise) },
                label = "title",
                modifier = Modifier.clearAndSetSemantics {
                    contentDescription = title
                    liveRegion = LiveRegionMode.Polite
                },
            ) { shown ->
                Text(
                    text = stringResource(heroTitle(shown)),
                    style = MaterialTheme.typography.headlineMediumEmphasized,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                )
            }
            AnimatedContent(targetState = hero, transitionSpec = { swapTransform(motion, rise) }, label = "detail") { shown ->
                Text(
                    text = heroDetail(shown, status, filteredApps, vpn),
                    style = MaterialTheme.typography.bodyMedium.copy(fontFeatureSettings = "tnum"),
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().padding(top = 2.dp).graphicsLayer { alpha = HERO_DETAIL_ALPHA },
                )
            }
            HeroAction(hero, actions, motion, rise)
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun HeroAction(hero: Hero, actions: HeroActions, motion: MotionScheme, rise: Int) {
    AnimatedContent(
        targetState = hero,
        transitionSpec = { swapTransform(motion, rise) },
        contentAlignment = Alignment.Center,
        label = "action",
        modifier = Modifier.padding(top = 18.dp),
    ) { shown ->
        when (shown) {
            Hero.Filtering -> PauseButton(actions.pause)

            Hero.Paused -> HeroButton(AdbIcons.PlayArrow, R.string.feature_overview_resume, actions.resume)

            Hero.Stopped -> HeroButton(AdbIcons.RestartAlt, R.string.feature_overview_start, actions.start)

            Hero.TurnedOff -> HeroButton(AdbIcons.RestartAlt, R.string.feature_overview_turn_on_again, actions.turnOnAgain)

            // Under five seconds: a loading indicator where the button goes.
            Hero.Connecting -> LoadingIndicator(Modifier.size(40.dp))

            Hero.Disconnected -> HeroButton(AdbIcons.VpnKey, R.string.feature_overview_reconnect, actions.reconnect)
        }
    }
}

private fun heroTitle(hero: Hero): Int = when (hero) {
    Hero.Filtering -> R.string.feature_overview_filtering
    Hero.Paused -> R.string.feature_overview_paused
    Hero.Stopped -> R.string.feature_overview_stopped
    Hero.TurnedOff -> R.string.feature_overview_turned_off
    Hero.Connecting -> R.string.feature_overview_connecting
    Hero.Disconnected -> R.string.feature_overview_disconnected
}

/** How far new words rise as they replace old ones. */
private val SWAP_RISE = 6.dp

/** The line under the status reads a little lighter than the status itself. */
private const val HERO_DETAIL_ALPHA = 0.86f

@Composable
private fun heroDetail(hero: Hero, status: DaemonStatus, filteredApps: Int, vpn: VpnStatus?): String = when (hero) {
    Hero.Filtering -> {
        val rules = status.rules ?: 0
        stringResource(
            // The method first, without root: what takes the apps' connections.
            if (vpn != null) R.string.feature_overview_filtering_detail_vpn else R.string.feature_overview_filtering_detail,
            pluralStringResource(R.plurals.feature_overview_apps, filteredApps, formatCount(filteredApps)),
            pluralStringResource(R.plurals.feature_overview_rules, rules, formatCount(rules)),
        )
    }

    Hero.Connecting -> stringResource(
        R.string.feature_overview_connecting_detail,
        pluralStringResource(R.plurals.feature_overview_apps, filteredApps, formatCount(filteredApps)),
    )

    // What it means for the chosen apps first: with lockdown, no network at all.
    Hero.Disconnected -> stringResource(
        when {
            vpn?.state == VpnState.Revoked -> R.string.feature_overview_vpn_revoked_detail
            vpn?.lockdown == true -> R.string.feature_overview_vpn_lockdown_detail
            vpn?.state == VpnState.Failed -> R.string.feature_overview_vpn_failed_detail
            else -> R.string.feature_overview_vpn_off_detail
        },
    )

    // The time left, counted down while shown, as the pause was chosen: "in 15 minutes".
    Hero.Paused -> status.resumeAt?.let { stringResource(R.string.feature_overview_resumes_in, timeLeft(it, rememberNow())) }
        ?: stringResource(R.string.feature_overview_paused_detail)

    Hero.Stopped -> stringResource(R.string.feature_overview_stopped_detail)

    Hero.TurnedOff -> stringResource(
        R.string.feature_overview_turned_off_detail,
        DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(LocalConfiguration.current.locales[0])
            .withZone(ZoneId.systemDefault()).format(status.bootGuard),
    )
}

@Composable
private fun HeroButton(icon: ImageVector, label: Int, onClick: () -> Unit) {
    Button(onClick = onClick) {
        Icon(icon, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
        Text(stringResource(label))
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PauseButton(onPause: (Int?) -> Unit) {
    var menuOpen by remember { mutableStateOf(false) }
    // The spec has the trailing icon turn on the standard motion scheme.
    val rotation by animateFloatAsState(if (menuOpen) 180f else 0f, MotionScheme.standard().fastSpatialSpec(), label = "chevron")
    val moreLabel = stringResource(R.string.feature_overview_more_durations)
    val expandedState = stringResource(if (menuOpen) R.string.feature_overview_expanded else R.string.feature_overview_collapsed)
    SplitButtonLayout(
        leadingButton = {
            SplitButtonDefaults.LeadingButton(onClick = { onPause(DEFAULT_PAUSE) }) {
                Icon(AdbIcons.Pause, contentDescription = null, Modifier.size(SplitButtonDefaults.LeadingIconSize))
                Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                Text(stringResource(R.string.feature_overview_pause_hour))
            }
        },
        trailingButton = {
            // The menu hangs from the trailing button, 4dp below it, and lines
            // up with it (M3 split buttons: align the menu with the trailing button).
            Box {
                SplitButtonDefaults.TrailingButton(
                    checked = menuOpen,
                    onCheckedChange = { menuOpen = it },
                    modifier = Modifier.semantics {
                        contentDescription = moreLabel
                        stateDescription = expandedState
                    },
                ) {
                    Icon(
                        AdbIcons.ExpandMore,
                        contentDescription = null,
                        modifier = Modifier.size(SplitButtonDefaults.TrailingIconSize).graphicsLayer { rotationZ = rotation },
                    )
                }
                DropdownMenuPopup(
                    expanded = menuOpen,
                    onDismissRequest = { menuOpen = false },
                    popupPositionProvider = MenuDefaults.rememberDropdownMenuPopupPositionProvider(MenuAnchorPosition.Below, DpOffset(0.dp, MENU_GAP)),
                ) {
                    DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
                        PAUSES.forEachIndexed { index, minutes ->
                            // The main action's duration is marked: the one a tap on it would choose.
                            SelectableDropdownMenuItem(
                                selected = minutes == DEFAULT_PAUSE,
                                onClick = {
                                    menuOpen = false
                                    onPause(minutes)
                                },
                                text = { Text(pauseLabel(minutes)) },
                                shapes = MenuDefaults.itemShape(index, PAUSES.size),
                                leadingIcon = { Icon(AdbIcons.Timer, contentDescription = null) },
                                selectedLeadingIcon = { Icon(AdbIcons.Timer, contentDescription = null) },
                            )
                        }
                    }
                }
            }
        },
    )
}

private val MENU_GAP = 4.dp

@Composable
private fun pauseLabel(minutes: Int?): String = when {
    minutes == null -> stringResource(R.string.feature_overview_until_resumed)
    minutes % 60 == 0 -> pluralStringResource(R.plurals.feature_overview_hours, minutes / 60, minutes / 60)
    else -> pluralStringResource(R.plurals.feature_overview_minutes, minutes, minutes)
}
