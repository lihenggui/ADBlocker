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
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.StatsSummary
import java.time.Clock
import javax.inject.Inject
import javax.inject.Provider

data class OverviewSnapshot(val status: DaemonStatus, val stats: StatsSummary, val lists: List<FilterList>)

interface OverviewRepository {
    suspend fun snapshot(): OverviewSnapshot
}

internal class AbdOverviewRepository @Inject constructor(
    private val abd: AbdClient,
    private val clock: Provider<Clock>,
    @Dispatcher(DEFAULT) private val defaultDispatcher: CoroutineDispatcher,
) : OverviewRepository {
    override suspend fun snapshot(): OverviewSnapshot = withContext(defaultDispatcher) {
        val period = StatsPeriod(clock.get())
        val overview = abd.overview(period.dayStarts)
        OverviewSnapshot(overview.status, period.summarize(overview.stats), overview.lists)
    }
}
