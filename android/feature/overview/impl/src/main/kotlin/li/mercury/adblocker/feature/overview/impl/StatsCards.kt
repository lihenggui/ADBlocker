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

import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.DayCount
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.RevealNotNull
import li.mercury.adblocker.core.ui.RollingNumber
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.formatCount
import li.mercury.adblocker.core.ui.profileAppLabel
import java.time.format.TextStyle

/** Today's count and the last seven days. A filled card: 12dp corners, surface container highest. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun TodayCard(stats: StatsSummary) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text(
                stringResource(R.string.feature_overview_blocked_today),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            RollingNumber(formatCount(stats.today.total), MaterialTheme.typography.displayLargeEmphasized)
            Text(
                stringResource(R.string.feature_overview_blocked_breakdown, formatCount(stats.today.requests), formatCount(stats.today.hosts)),
                style = MaterialTheme.typography.bodyMedium.copy(fontFeatureSettings = "tnum"),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            RevealNotNull(stats.week.ifEmpty { null }) { week -> WeekBars(week, Modifier.padding(top = 14.dp)) }
        }
    }
}

/** The tallest a day's bar is drawn. */
private const val BAR_MAX_DP = 70

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun WeekBars(week: List<DayCount>, modifier: Modifier = Modifier) {
    val max = week.maxOf { it.blocked.total }.coerceAtLeast(1)
    // Follows a change of language while the screen is shown.
    val locale = LocalConfiguration.current.locales[0]
    val today = stringResource(R.string.feature_overview_today)
    val description = stringResource(
        R.string.feature_overview_week_description,
        week.joinToString { "${it.date.dayOfWeek.getDisplayName(TextStyle.FULL, locale)} ${it.blocked.total}" },
    )
    Row(
        modifier.fillMaxWidth().height(92.dp).semantics(mergeDescendants = true) { contentDescription = description },
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        week.forEachIndexed { index, day ->
            val isToday = index == week.lastIndex
            val height by animateDpAsState(
                (day.blocked.total.toFloat() / max * BAR_MAX_DP).dp.coerceAtLeast(6.dp),
                MaterialTheme.motionScheme.defaultSpatialSpec(),
                label = "bar",
            )
            Column(Modifier.weight(1f).fillMaxHeight(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Bottom) {
                Box(
                    Modifier
                        .widthIn(max = 28.dp)
                        .fillMaxWidth()
                        .height(height)
                        .background(
                            // Outline has 3:1 contrast on surfaces, as a graphic that carries data needs.
                            if (isToday) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
                            RoundedCornerShape(6.dp),
                        ),
                )
                // A label may be wider than its bar, as "Today" is in a narrow
                // card: it reaches into the gaps rather than being cut.
                Text(
                    text = if (isToday) today else day.date.dayOfWeek.getDisplayName(TextStyle.SHORT, locale),
                    style = MaterialTheme.typography.labelSmall,
                    fontWeight = if (isToday) FontWeight.Bold else null,
                    color = if (isToday) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    softWrap = false,
                    modifier = Modifier.padding(top = 6.dp).wrapContentWidth(unbounded = true),
                )
            }
        }
    }
}

@Composable
internal fun TopApps(apps: List<Pair<AppEntry, Long>>, onOpenApp: (String) -> Unit) {
    val max = apps.maxOf { it.second }.coerceAtLeast(1)
    SegmentedGroup(apps.size) { index ->
        val (entry, count) = apps[index]
        SegmentedItem(
            index = index,
            count = apps.size,
            headline = profileAppLabel(entry.app.label, entry.app.profileSerial != null, entry.app.isSharedGroup),
            leading = { AppIcon(entry.app, entry.app.label) },
            supporting = {
                Box(
                    Modifier.padding(top = 6.dp).fillMaxWidth().height(4.dp)
                        .background(MaterialTheme.colorScheme.surfaceContainerHighest, RoundedCornerShape(2.dp)),
                ) {
                    Box(
                        Modifier.fillMaxWidth(count.toFloat() / max).fillMaxHeight()
                            .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(2.dp)),
                    )
                }
            },
            trailing = {
                Text(
                    formatCount(count),
                    style = MaterialTheme.typography.labelMedium.copy(fontFeatureSettings = "tnum"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            },
            onClick = { onOpenApp(entry.appKey) },
        )
    }
}
