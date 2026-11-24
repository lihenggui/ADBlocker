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

import android.net.Uri
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.RootShell
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class RootDiagnosticsRepositoryTest {
    @get:Rule
    val temporary = TemporaryFolder()

    private val application get() = RuntimeEnvironment.getApplication()
    private var readDaemon: () -> CommandResult = { CommandResult(true, 0, "== abd.log\ndaemon started", "") }
    private val shell = object : RootShell(Dispatchers.Unconfined) {
        override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
            assertEquals("log export", label)
            return readDaemon()
        }
    }

    private val truncatingWriter = DocumentWriter { uri, bytes -> File(requireNotNull(uri.path)).writeBytes(bytes) }

    @Test
    fun givenTheDaemonLogIsIncomplete_whenExported_thenTheChosenDocumentStillContainsTheAppAndReadableDaemonLogs() = runTest {
        val logs = application.filesDir.resolve("logs")
        assertTrue(logs.mkdirs())
        logs.resolve("current").writeText("app started")
        readDaemon = { CommandResult(false, 1, "== abd.log\nprevious session ended", "private shell diagnostics") }
        val destination = temporary.newFile()
        val repository = RootDiagnosticsRepository(application, shell, truncatingWriter, StandardTestDispatcher(testScheduler))

        assertEquals(LogExportResult.DaemonIncomplete, repository.exportLogs(Uri.fromFile(destination)))
        assertEquals(
            listOf("== abd.log", "previous session ended", "== app", "app started"),
            destination.readLines(),
        )
    }

    @Test
    fun givenALogExport_whenWritten_thenItGoesToTheDocumentWriterInOnePiece() = runTest {
        val destination = Uri.fromFile(temporary.newFile())
        val writes = mutableListOf<Pair<Uri, String>>()
        val writer = DocumentWriter { uri, bytes -> writes += uri to bytes.decodeToString() }
        val repository = RootDiagnosticsRepository(application, shell, writer, StandardTestDispatcher(testScheduler))

        repository.exportLogs(destination)

        assertEquals(listOf(destination to "== abd.log\ndaemon started\n== app\n\n"), writes)
    }

    @Test
    fun givenReadingTheDaemonLogIsCancelled_whenExported_thenTheDestinationIsUntouched() = runTest {
        readDaemon = { throw CancellationException("export cancelled") }
        val destination = temporary.newFile().apply { writeText("existing document") }
        val repository = RootDiagnosticsRepository(application, shell, truncatingWriter, StandardTestDispatcher(testScheduler))

        assertFailsWith<CancellationException> { repository.exportLogs(Uri.fromFile(destination)) }
        assertEquals("existing document", destination.readText())
    }
}
