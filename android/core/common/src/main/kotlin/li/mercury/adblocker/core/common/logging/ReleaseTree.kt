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

import android.annotation.SuppressLint
import android.util.Log
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.core.common.di.FilesDir
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import timber.log.Timber
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.format.DateTimeFormatter
import java.util.Collections
import java.util.IdentityHashMap
import javax.inject.Inject
import javax.inject.Singleton

/** The directory, in the app's files, that holds the log files. */
const val LOG_DIR = "logs"

/**
 * Keeps the app's log in files, one per day for the last [DAYS_KEPT] days,
 * so that a problem can be looked into after it happened. Lines are written
 * in the order they were logged, by one writer off the main thread, except
 * a crash, which is written before the process ends. Verbose lines are left
 * out.
 *
 * What the app records (paths, package names, search text) must not be
 * logged: only what the app did, how long it took and how it ended.
 */
@Singleton
class ReleaseTree internal constructor(
    filesDir: File,
    scope: CoroutineScope,
    ioDispatcher: CoroutineDispatcher,
    private val today: () -> LocalDate,
    private val now: () -> LocalDateTime,
) : Timber.Tree() {
    @Inject
    constructor(
        @FilesDir filesDir: File,
        @ApplicationScope scope: CoroutineScope,
        @Dispatcher(IO) ioDispatcher: CoroutineDispatcher,
    ) : this(filesDir, scope, ioDispatcher, LocalDate::now, LocalDateTime::now)

    private val directory = File(filesDir, LOG_DIR)
    private val lines = Channel<String>(MAX_PENDING_LINES, BufferOverflow.DROP_OLDEST)
    private val writes = Channel<Unit>(Channel.CONFLATED)
    private var lastDay: LocalDate? = null
    private var reportedWriteFailure = false

    init {
        scope.launch(ioDispatcher) {
            directory.mkdirs()
            deleteOldFiles()
            for (ignored in writes) appendBatch()
        }
    }

    override fun isLoggable(tag: String?, priority: Int): Boolean = priority > Log.VERBOSE

    override fun log(priority: Int, tag: String?, message: String, t: Throwable?) {
        // Timber has already appended the stack trace of t to the message.
        lines.trySend(format(priority, tag, message))
        writes.trySend(Unit)
    }

    /**
     * Writes an uncaught exception at once, on the crashing thread, after
     * what was logged before it: the process ends right after. Only its
     * classes and frames are written (see [withoutMessages]).
     */
    fun logCrash(thread: Thread, error: Throwable) {
        appendBatch(crash = format(Log.ERROR, null, "Crash on thread ${thread.name}\n${error.withoutMessages()}"))
    }

    private fun format(priority: Int, tag: String?, message: String): String {
        val level = when (priority) {
            Log.DEBUG -> 'D'
            Log.INFO -> 'I'
            Log.WARN -> 'W'
            Log.ERROR -> 'E'
            else -> 'A'
        }
        val time = now().format(TIME_FORMAT)
        val prefix = if (tag == null) "$time $level " else "$time $level/${tag.take(MAX_TAG_CHARS)} "
        return prefix + message.take(MAX_ENTRY_CHARS - prefix.length)
    }

    // The log can't report its own failure through itself.
    @SuppressLint("LogNotTimber")
    @Synchronized
    private fun appendBatch(crash: String? = null) {
        val first = lines.tryReceive().getOrNull()
        if (first == null && crash == null) return
        try {
            directory.mkdirs()
            val day = today()
            val file = File(directory, "$day.log")
            var remaining = MAX_FILE_BYTES - file.length()
            val batch = sequence {
                if (first != null) yield(first)
                repeat(MAX_PENDING_LINES - 1) {
                    yield(lines.tryReceive().getOrNull() ?: return@sequence)
                }
            }
            FileOutputStream(file, true).buffered().use { out ->
                // Leave room for the crash report even after ordinary logging reaches its limit.
                for (line in batch) {
                    remaining -= writeLine(out, line, remaining - MAX_ENTRY_BYTES)
                }
                if (crash != null) writeLine(out, crash, remaining)
            }
            if (lastDay != day) {
                lastDay = day
                deleteOldFiles(keep = file)
            }
        } catch (e: IOException) {
            if (!reportedWriteFailure) {
                reportedWriteFailure = true
                Log.e(LOG_TAG, "Could not write the log: ${e.javaClass.simpleName}")
            }
        }
    }

    private fun writeLine(out: BufferedOutputStream, line: String, remaining: Long): Int {
        if (remaining <= 0) return 0
        val bytes = (line + "\n").toByteArray()
        if (bytes.size > remaining) return 0
        out.write(bytes)
        return bytes.size
    }

    /** Files are named by date, so the oldest sort first. */
    private fun deleteOldFiles(keep: File? = null) {
        val files = directory.listFiles { file -> file != keep && file.name.endsWith(".log") } ?: return
        val otherDays = if (keep == null) DAYS_KEPT else DAYS_KEPT - 1
        // A file that could not be removed is tried again at the next rollover or startup.
        files.sortedBy { it.name }.dropLast(otherDays).forEach { it.delete() }
    }

    internal companion object {
        const val DAYS_KEPT = 7
        internal const val MAX_PENDING_LINES = 256
        internal const val MAX_FILE_BYTES = 2 * 1024 * 1024L
        private const val MAX_TAG_CHARS = 64
        private const val MAX_ENTRY_CHARS = 8 * 1024
        private const val MAX_ENTRY_BYTES = MAX_ENTRY_CHARS * 3 + 1
        private const val LOG_TAG = "ReleaseTree"
        private val TIME_FORMAT: DateTimeFormatter = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss.SSS")
    }
}

/**
 * The stack trace of this exception and of its causes, without their
 * messages: a message can carry what an app talks to, such as a host in the
 * output of a command that could not be read, or a package name.
 */
internal fun Throwable.withoutMessages(): String = buildString {
    val seen = Collections.newSetFromMap(IdentityHashMap<Throwable, Boolean>())
    var current: Throwable? = this@withoutMessages
    while (current != null && seen.add(current)) {
        if (current !== this@withoutMessages) append("Caused by: ")
        appendLine(current.javaClass.name)
        current.stackTrace.forEach { appendLine("\tat $it") }
        current.suppressed.forEach { appendLine("\tSuppressed: ${it.javaClass.name}") }
        current = current.cause
    }
}.trimEnd()
