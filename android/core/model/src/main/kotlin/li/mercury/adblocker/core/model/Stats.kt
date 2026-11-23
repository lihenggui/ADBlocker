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

package li.mercury.adblocker.core.model

import java.time.LocalDate

/** Seven days of blocked counts in the order requested from `abd stats --json`. */
data class DailyStats(
    val days: List<BlockedCount>,
    val apps: Map<String, List<BlockedCount>>,
)

/** Blocked in two ways: requests inside decrypted connections, and whole connections refused for their host. */
data class BlockedCount(val requests: Long, val hosts: Long) {
    val total: Long get() = requests.saturatingAdd(hosts)

    operator fun plus(other: BlockedCount) = BlockedCount(requests.saturatingAdd(other.requests), hosts.saturatingAdd(other.hosts))

    companion object {
        val Zero = BlockedCount(0, 0)
    }
}

private fun Long.saturatingAdd(other: Long): Long = if (this > Long.MAX_VALUE - other) Long.MAX_VALUE else this + other

data class DayCount(val date: LocalDate, val blocked: BlockedCount)

/** One app's counts, in days of the device's time zone. */
data class AppCount(
    val today: BlockedCount,
    val week: BlockedCount,
    /** The last seven days, oldest first, today last. */
    val days: List<Long>,
)

/** The counts as the overview and the app pages show them. */
data class StatsSummary(
    val today: BlockedCount,
    /** The last seven days, oldest first, today last. */
    val week: List<DayCount>,
    val perApp: Map<String, AppCount>,
) {
    companion object {
        val Empty = StatsSummary(BlockedCount.Zero, emptyList(), emptyMap())
    }
}
