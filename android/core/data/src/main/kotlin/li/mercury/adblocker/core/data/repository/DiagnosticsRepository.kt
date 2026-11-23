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

import android.content.Context
import android.net.Uri
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.root.ModulePaths.LOG
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.shellQuoted
import java.io.File
import java.io.OutputStream
import javax.inject.Inject

/** The daemon's log, for a bug report. It says what ran and how it ended, never what apps talk to. */
interface DiagnosticsRepository {
    /** Writes the app's log and any available daemon logs into the document the user chose. */
    suspend fun exportLogs(destination: Uri): LogExportResult
}

enum class LogExportResult { Complete, DaemonIncomplete }

internal class RootDiagnosticsRepository @Inject constructor(
    @ApplicationContext private val context: Context,
    private val rootShell: RootShell,
    @Dispatcher(IO) private val ioDispatcher: CoroutineDispatcher,
) : DiagnosticsRepository {
    override suspend fun exportLogs(destination: Uri): LogExportResult = withContext(ioDispatcher) {
        exportLogFile(rootShell, context.filesDir.resolve("logs")) { context.contentResolver.openOutputStream(destination) }
    }
}

internal suspend fun exportLogFile(rootShell: RootShell, appLogs: File, openDestination: () -> OutputStream?): LogExportResult {
    val current = LOG.shellQuoted()
    val history = "$LOG.old".shellQuoted()
    val command = """
        failed=0
        if [ -e $history ] || [ -L $history ]; then
            tail -n 2000 $history 2>/dev/null || failed=1
        fi
        tail -n 4000 $current 2>/dev/null || failed=1
        exit "${'$'}failed"
    """.trimIndent()
    val daemon = suspendRunCatching { rootShell.run(command, label = "log export") }.getOrNull()
    val app = appLogs.listFiles().orEmpty().sortedBy { it.name }.joinToString("\n") { it.readText() }
    val out = openDestination() ?: throw AbdException.CommandFailed("cannot write the file")
    out.bufferedWriter().use {
        it.appendLine("== abd.log")
        it.appendLine(daemon?.output.orEmpty())
        it.appendLine("== app")
        it.appendLine(app)
    }
    return if (daemon?.isSuccess == true) LogExportResult.Complete else LogExportResult.DaemonIncomplete
}
