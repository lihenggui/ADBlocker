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

package li.mercury.adblocker.core.common.logging

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.time.LocalDate
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class ReleaseTreeTest {
    @get:Rule
    val folder = TemporaryFolder()

    private var day = LocalDate.of(2026, 11, 25)
    private val directory get() = folder.root.resolve(LOG_DIR)
    private val log get() = directory.resolve("$day.log")

    private fun TestScope.tree() = ReleaseTree(
        folder.root,
        backgroundScope,
        StandardTestDispatcher(testScheduler),
        { day },
        { day.atStartOfDay() },
    )

    @Test
    fun givenTheWriterHasNotRun_whenTheQueueOverflows_thenItKeepsOnlyTheNewestEntries() = runTest {
        val tree = tree()
        val count = ReleaseTree.MAX_PENDING_LINES

        repeat(count + 30) { tree.w("entry %d", it) }
        runCurrent()

        val lines = log.readLines()
        assertEquals(count, lines.size)
        assertTrue(lines.first().endsWith("entry 30"))
        assertTrue(lines.last().endsWith("entry ${count + 29}"))
    }

    @Test
    fun givenAFullDayOfLogs_whenTheAppCrashes_thenTheFileStaysCappedAndKeepsTheSanitizedCrash() = runTest {
        val tree = tree()
        val message = "界".repeat(9000)

        repeat(150) { tree.w(message) }
        runCurrent()
        assertTrue(log.length() in 1..ReleaseTree.MAX_FILE_BYTES)

        tree.logCrash(Thread.currentThread(), IllegalStateException("private message"))

        assertTrue(log.length() <= ReleaseTree.MAX_FILE_BYTES)
        assertTrue(log.readText().contains("java.lang.IllegalStateException"))
        assertFalse(log.readText().contains("private message"))
    }

    @Test
    fun givenSevenDailyLogs_whenTheDateRollsOver_thenTheOldestIsRemoved() = runTest {
        val tree = tree()
        val oldest = day
        repeat(ReleaseTree.DAYS_KEPT + 1) {
            day = oldest.plusDays(it.toLong())
            tree.i("day %d", it)
            runCurrent()
        }

        assertEquals(ReleaseTree.DAYS_KEPT, directory.listFiles()?.size)
        assertFalse(directory.resolve("$oldest.log").exists())
        assertTrue(log.readText().contains("day ${ReleaseTree.DAYS_KEPT}"))
    }
}
