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

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.ModulePaths.LOG
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.sh
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class DiagnosticsRepositoryTest {
    @get:Rule
    val temporary = TemporaryFolder()

    private val current get() = temporary.root.resolve("abd.log")
    private val history get() = temporary.root.resolve("abd.log.old")
    private var rootGranted = true
    private val shell = object : RootShell(Dispatchers.IO) {
        override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
            if (!rootGranted) throw AbdException.NoRoot()
            assertEquals("log export", label)
            val result = sh(command.replace(LOG, current.path))
            return CommandResult(result.code == 0, result.code, result.output, "")
        }
    }

    private suspend fun export(): Pair<LogExportResult, String> {
        val logs = temporary.newFolder("applogs")
        logs.resolve("current").writeText("app started")
        val output = ByteArrayOutputStream()
        return exportLogFile(shell, logs) { output.write(it) } to output.toString(Charsets.UTF_8)
    }

    @Test
    fun givenOnlyTheCurrentDaemonLogExists_whenExporting_thenMissingHistoryIsOptional() = runTest {
        current.writeText("daemon started")

        val (result, text) = export()

        assertEquals(LogExportResult.Complete, result)
        assertTrue("daemon started" in text)
        assertTrue("app started" in text)
    }

    @Test
    fun givenTheCurrentDaemonLogIsUnreadable_whenExporting_thenTheAppAndReadableHistoryAreKept() = runTest {
        assertTrue(current.mkdir())
        history.writeText("previous session ended")

        val (result, text) = export()

        assertEquals(LogExportResult.DaemonIncomplete, result)
        assertTrue("previous session ended" in text)
        assertTrue("app started" in text)
    }

    @Test
    fun givenTheOldDaemonLogExistsButIsUnreadable_whenExporting_thenTheCurrentLogIsKeptWithAnIncompleteResult() = runTest {
        current.writeText("daemon started")
        assertTrue(history.mkdir())

        val (result, text) = export()

        assertEquals(LogExportResult.DaemonIncomplete, result)
        assertTrue("daemon started" in text)
        assertTrue("app started" in text)
    }

    @Test
    fun givenRootIsDenied_whenExporting_thenTheAppLogIsStillWritten() = runTest {
        rootGranted = false

        val (result, text) = export()

        assertEquals(LogExportResult.DaemonIncomplete, result)
        assertTrue("app started" in text)
    }
}
