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

package li.mercury.adblocker.core.data.repository

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.DEFAULT
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.protocol.STATS_DAYS
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.model.DailyStats
import li.mercury.adblocker.core.model.DayCount
import li.mercury.adblocker.core.model.StatsSummary
import java.time.Clock
import javax.inject.Inject
import javax.inject.Provider

/** What was blocked, in days of the device's time zone. */
interface StatsRepository {
    suspend fun summary(): StatsSummary
}

internal class AbdStatsRepository @Inject constructor(
    private val abd: AbdClient,
    private val clock: Provider<Clock>,
    @Dispatcher(DEFAULT) private val defaultDispatcher: CoroutineDispatcher,
) : StatsRepository {
    override suspend fun summary(): StatsSummary = withContext(defaultDispatcher) {
        val period = StatsPeriod(clock.get())
        period.summarize(abd.stats(period.dayStarts))
    }
}

/** Capture the dates before the command to keep the labels aligned with the cutoffs. */
internal class StatsPeriod(clock: Clock) {
    private val zone = clock.zone
    private val first = clock.instant().atZone(zone).toLocalDate().minusDays(STATS_DAYS - 1L)
    private val dates = List(STATS_DAYS + 1) { first.plusDays(it.toLong()) }
    val dayStarts = dates.map { it.atStartOfDay(zone).toEpochSecond() }

    fun summarize(stats: DailyStats) = StatsSummary(
        today = stats.days.last(),
        week = stats.days.mapIndexed { index, blocked -> DayCount(dates[index], blocked) },
        perApp = stats.apps.mapValues { (_, days) ->
            AppCount(
                today = days.last(),
                week = days.fold(BlockedCount.Zero, BlockedCount::plus),
                days = days.map(BlockedCount::total),
            )
        },
    )
}
