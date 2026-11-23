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
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.transport.AbdTransport
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
import kotlin.test.assertNull

class OverviewRepositoryTest {
    @Test
    fun oneOverviewCommandReadsTheSnapshotWithDaylightSavingCutoffs() = runTest {
        val cases = File(System.getProperty("protocolCases") ?: error("protocolCases is set by core/data/build.gradle.kts"))
        val clock = Clock.fixed(Instant.parse("2026-03-09T18:00:00Z"), ZoneId.of("America/Los_Angeles"))
        val commands = mutableListOf<List<String>>()
        val transport = object : AbdTransport by unexpected() {
            override suspend fun run(args: List<String>, label: String, input: String?): String {
                assertEquals("overview", label)
                assertNull(input)
                commands.add(args)
                return cases.resolve("overview.json").readText()
            }
        }
        val repository = AbdOverviewRepository(transport.fixedAbdClient(), Provider { clock }, StandardTestDispatcher(testScheduler))

        val snapshot = repository.snapshot()
        val cutoffs = (3..8).map { "2026-03-0${it}T08:00:00Z" } + listOf("2026-03-09T07:00:00Z", "2026-03-10T07:00:00Z")
        assertEquals(listOf(listOf("overview", "--json") + cutoffs.map { Instant.parse(it).epochSecond.toString() }), commands)
        assertEquals((3..9).map { LocalDate.of(2026, 3, it) }, snapshot.stats.week.map { it.date })
        assertEquals(BlockedCount(184, 52), snapshot.stats.today)
        assertEquals(BlockedCount(188, 43), snapshot.stats.perApp.getValue("com.example.news").week)
        assertEquals(4242, snapshot.status.pid)
        assertEquals(2, snapshot.lists.count { it.enabled })
    }
}
