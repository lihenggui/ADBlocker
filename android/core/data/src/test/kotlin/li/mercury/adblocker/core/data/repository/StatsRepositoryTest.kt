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

import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.model.AppCount
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Test
import java.io.File
import java.time.Clock
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import javax.inject.Provider
import kotlin.test.assertEquals

class StatsRepositoryTest {
    private val cases = File(System.getProperty("protocolCases") ?: error("protocolCases is set by core/data/build.gradle.kts"))
    private val stats = cases.resolve("stats.json").readText()

    private fun TestScope.repository(clock: () -> Clock, answer: (List<String>) -> String): StatsRepository {
        val transport = object : AbdTransport by unexpected() {
            override suspend fun run(args: List<String>, label: String, input: String?): String {
                assertEquals("stats", label)
                return answer(args)
            }
        }
        return AbdStatsRepository(transport.fixedAbdClient(), Provider { clock() }, StandardTestDispatcher(testScheduler))
    }

    @Test
    fun theSevenDaysStartAtLocalMidnightInAFractionalOffsetZone() = runTest {
        val clock = Clock.fixed(Instant.parse("2025-11-19T18:20:00Z"), ZoneId.of("Asia/Kathmandu"))
        val repository = repository({ clock }) { args ->
            val expected = (13..20).map { Instant.parse("2025-11-${it}T18:15:00Z").epochSecond.toString() }
            assertEquals(listOf("stats", "--json") + expected, args)
            stats
        }

        val summary = repository.summary()
        assertEquals((14..20).map { LocalDate.of(2025, 11, it) }, summary.week.map { it.date })
        assertEquals(listOf(4L, 0L, 0L, 0L, 0L, 0L, 236L), summary.week.map { it.blocked.total })
        assertEquals(BlockedCount(184, 52), summary.today)
        assertEquals(
            AppCount(BlockedCount(184, 43), BlockedCount(188, 43), listOf(4L, 0L, 0L, 0L, 0L, 0L, 227L)),
            summary.perApp.getValue("com.example.news"),
        )
        assertEquals(BlockedCount(0, 9), summary.perApp.getValue("com.example.shop").week)
    }

    @Test
    fun localMidnightsAccountForShorterAndLongerDays() = runTest {
        val zone = ZoneId.of("America/Los_Angeles")
        for ((instant, hours) in listOf("2026-03-09T18:00:00Z" to 23L, "2026-11-02T18:00:00Z" to 25L)) {
            val clock = Clock.fixed(Instant.parse(instant), zone)
            repository({ clock }) { args ->
                val starts = args.drop(2).map(String::toLong)
                val durations = starts.zipWithNext { start, end -> end - start }
                assertEquals(listOf(24L, 24L, 24L, 24L, 24L, hours, 24L).map { it * 3600 }, durations)
                stats
            }.summary()
        }
    }

    @Test
    fun aTimeZoneChangeAffectsTheNextReadAndKeepsDatesAlignedWithBothResponses() = runTest {
        val now = Instant.parse("2025-11-19T18:20:00Z")
        var clock = Clock.fixed(now, ZoneId.of("UTC"))
        val startsOfToday = mutableListOf<Long>()
        val repository = repository({ clock }) { args ->
            startsOfToday += args[args.lastIndex - 1].toLong()
            clock = Clock.fixed(now, ZoneId.of("Asia/Kathmandu"))
            stats
        }
        assertEquals(LocalDate.of(2025, 11, 19), repository.summary().week.last().date)
        assertEquals(LocalDate.of(2025, 11, 20), repository.summary().week.last().date)
        assertEquals(
            listOf("2025-11-19T00:00:00Z", "2025-11-19T18:15:00Z").map { Instant.parse(it).epochSecond },
            startsOfToday,
        )
    }

    @Test
    fun aSkippedLocalDateHasAnEmptyInterval() = runTest {
        val clock = Clock.fixed(Instant.parse("2011-12-31T03:00:00Z"), ZoneId.of("Pacific/Apia"))
        val summary = repository({ clock }) { args ->
            val starts = args.drop(2)
            assertEquals(starts[5], starts[6])
            stats
        }.summary()
        assertEquals(LocalDate.of(2011, 12, 30), summary.week[5].date)
        assertEquals(BlockedCount.Zero, summary.week[5].blocked)
    }

    @Test
    fun combiningLargeCountsSaturatesTheChartAndWeeklyTotals() = runTest {
        val clock = Clock.fixed(Instant.parse("2025-11-19T18:20:00Z"), ZoneId.of("UTC"))
        val max = Long.MAX_VALUE
        val days = (listOf("[1,$max]") + List(5) { "[0,0]" } + "[$max,1]").joinToString(",", "[", "]")
        val summary = repository({ clock }) {
            """{"days":$days,"apps":{"com.example.news":$days}}"""
        }.summary()
        val news = summary.perApp.getValue("com.example.news")
        assertEquals(max, summary.today.total)
        assertEquals(BlockedCount(max, max), news.week)
        assertEquals(listOf(max, 0L, 0L, 0L, 0L, 0L, max), news.days)
    }
}
