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

package li.mercury.adblocker.core.data.root

import android.os.SystemClock
import com.topjohnwu.superuser.Shell
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.channels.trySendBlocking
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.data.AbdException
import timber.log.Timber
import java.io.IOException
import java.util.concurrent.TimeUnit
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.concurrent.thread

/** Result of a root command. */
class CommandResult(val isSuccess: Boolean, val code: Int, val output: String, val error: String)

/**
 * Runs commands as root. The single place the app touches root; open so that
 * the repositories' tests can answer in its place.
 */
@Singleton
open class RootShell @Inject constructor(
    @Dispatcher(IO) private val ioDispatcher: CoroutineDispatcher,
) {
    /**
     * Runs [command] in the shared root shell, logging how long it took under
     * [label]. The label is all that is logged of it: commands carry package
     * names, hosts and rules.
     *
     * The command is stopped after [timeoutSeconds]: the shell is shared, and
     * one command that never ends would hold up every other, the Quick
     * Settings tile's included. Cancelling the caller does not stop it.
     *
     * @throws AbdException.NoRoot if the app has not been granted root.
     * @throws AbdException.TimedOut if it ran for longer than [timeoutSeconds].
     */
    open suspend fun run(
        command: String,
        label: String = command.substringBefore(' '),
        timeoutSeconds: Int = DEFAULT_TIMEOUT_SECONDS,
    ): CommandResult = withContext(ioDispatcher) {
        // A job only captures output into lists it is given.
        val output = ArrayList<String>()
        val error = ArrayList<String>()
        val start = SystemClock.elapsedRealtime()
        val shell = rootShell()
        val opened = SystemClock.elapsedRealtime()
        val result = shell.newJob().add(bounded(command, timeoutSeconds)).to(output, error).exec()
        val elapsed = SystemClock.elapsedRealtime() - opened
        // Opening the shell is slow once, then free.
        val opening = if (opened - start > 0) " after ${opened - start} ms to open the shell" else ""
        Timber.d("%s: exit %d in %d ms, %d lines%s", label, result.code, elapsed, output.size, opening)
        if (result.code == TIMED_OUT) throw AbdException.TimedOut()
        CommandResult(
            isSuccess = result.isSuccess,
            code = result.code,
            output = output.joinToString("\n"),
            error = error.joinToString("\n"),
        )
    }

    /** The payload goes directly to the child's stdin, including its exact final newline. */
    open suspend fun runWithInput(command: String, label: String, input: String): CommandResult = withContext(ioDispatcher) {
        rootShell()
        // Once the writer starts, finish the bounded command even if its screen closes.
        // Closing stdin on cancellation would let rules set commit a partial file.
        withContext(NonCancellable) {
            val process = ProcessBuilder("su", "--mount-master", "-c", "exec timeout $DEFAULT_TIMEOUT_SECONDS $command").start()
            val result = communicate(process, input, ioDispatcher)
            Timber.d("%s: exit %d", label, result.code)
            if (result.code == TIMED_OUT) throw AbdException.TimedOut()
            result
        }
    }

    /**
     * Runs [command] as root in a process of its own and emits what it prints,
     * a line at a time, as it prints it. For commands that report as they go
     * (`abd update --json`) or until they are stopped (`abd watch --json`):
     * in the shared shell they would hold up every other command meanwhile.
     *
     * When the collector stops, the process's standard input is closed, which
     * the command must take as its cue to exit (`abd watch` does; see
     * [endingWithInput] for one that does not), and the process is then
     * destroyed. A collector that falls behind holds the reading up rather
     * than lose lines: the daemon counts what it leaves out for a reader too
     * slow. A command exit failure keeps its status, and an incomplete read
     * is reported even if the command exits successfully.
     */
    open fun stream(command: String, label: String): Flow<String> = streamProcess(label) {
        rootShell()
        // The same global mount namespace as the shared shell.
        ProcessBuilder("su", "--mount-master", "-c", command).start()
    }.flowOn(ioDispatcher)

    /**
     * A shell with root, or [AbdException.NoRoot].
     *
     * libsu caches the shell it opens. If root was denied when that happened,
     * the cached shell stays unprivileged even after the user grants root, so
     * an unprivileged shell is closed and a new one requested each time; the
     * new one picks up a grant made in the meantime.
     */
    private fun rootShell(): Shell {
        val shell = try {
            val cached = Shell.getShell()
            if (cached.isRoot) return cached
            cached.close()
            Shell.getShell()
        } catch (_: Exception) {
            // libsu could not start a shell at all.
            throw AbdException.NoRoot()
        }
        if (!shell.isRoot) throw AbdException.NoRoot()
        return shell
    }

    companion object {
        /** How long a command may run unless its caller says otherwise. */
        const val DEFAULT_TIMEOUT_SECONDS = 60

        /**
         * Sets libsu up before any shell is opened; the application calls it
         * first thing. The root shell runs in the global mount namespace, as
         * the module's boot script does: a daemon started again from the app
         * must not live in the app's own namespace. libsu falls back to a
         * plain `su` where `su --mount-master` gives no root.
         */
        fun configure() {
            Shell.setDefaultBuilder(Shell.Builder.create().setFlags(Shell.FLAG_MOUNT_MASTER))
        }
    }
}

private const val MAX_STDERR = 16 * 1024
private const val STDERR_JOIN_MS = 1_000L
private const val PIPE_FAILURE_WAIT_MS = 1_000L

internal fun streamProcess(label: String, start: () -> Process): Flow<String> = callbackFlow {
    val process = start()
    val stderr = StringBuffer()
    val errors = thread(name = "$label-stderr", isDaemon = true) {
        runCatching {
            process.errorStream.bufferedReader().use { input ->
                val buffer = CharArray(1024)
                while (true) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    stderr.append(buffer, 0, count.coerceAtMost(MAX_STDERR - stderr.length))
                }
            }
        }
    }
    val reader = thread(name = label, isDaemon = true) {
        try {
            val readFailure = try {
                process.inputStream.bufferedReader().use { input ->
                    while (true) {
                        val line = input.readLine() ?: break
                        if (trySendBlocking(line).isFailure) return@thread
                    }
                }
                null
            } catch (e: IOException) {
                e
            }
            val code = when {
                readFailure == null -> process.waitFor()

                // A broken pipe can leave a watcher alive indefinitely. Give the
                // child a chance to supply its own status before closing stdin.
                process.waitFor(PIPE_FAILURE_WAIT_MS, TimeUnit.MILLISECONDS) -> process.exitValue()

                else -> null
            }
            Timber.d("%s: exit %s", label, code?.toString() ?: "unavailable")
            if (code != null && code != 0) {
                errors.join(STDERR_JOIN_MS)
                close(exitFailure(code, stderr.toString()))
            } else {
                close(readFailure)
            }
        } catch (_: InterruptedException) {
            // The collector has already closed the stream.
        }
    }
    awaitClose {
        runCatching { process.outputStream.close() }
        process.destroy()
        reader.interrupt()
    }
}

/** Drain both pipes while writing. A checker can report more than a pipeful of issues. */
internal suspend fun communicate(process: Process, input: String, ioDispatcher: CoroutineDispatcher): CommandResult = withContext(ioDispatcher) {
    try {
        val stdout = async { process.inputStream.bufferedReader().use { it.readText() } }
        val stderr = async { process.errorStream.bufferedReader().use { it.readText() } }
        val writeFailure = try {
            process.outputStream.bufferedWriter().use { it.write(input) }
            null
        } catch (e: IOException) {
            // A missing executable or a timeout can close stdin before the payload is written.
            e
        }
        val code = process.waitFor()
        if (code == 0 && writeFailure != null) throw writeFailure
        CommandResult(code == 0, code, stdout.await(), stderr.await())
    } finally {
        process.destroy()
    }
}

/** What toybox `timeout` exits with when it stopped the command. */
internal const val TIMED_OUT = 124

/**
 * [command] in a shell of its own, stopped after [seconds]. Its standard input
 * is empty: the shared shell reads the next commands from its own, which a
 * command reading it would take.
 */
internal fun bounded(command: String, seconds: Int): String = "timeout $seconds sh -c ${command.shellQuoted()} </dev/null"

/** The exception for an abd command that exited with [code]. */
internal fun exitFailure(code: Int, output: String): AbdException = when (code) {
    // The shell could not find abd: no module.
    126, 127 -> AbdException.ModuleMissing()

    // abd's usage error: a command or option it does not know.
    2 -> AbdException.CommandRejected()

    TIMED_OUT -> AbdException.TimedOut()

    else -> AbdException.CommandFailed(output.trim().ifEmpty { "exit status $code" })
}

/**
 * This string as a single shell word. Inside single quotes the shell
 * interprets nothing, so the only character needing care is the single quote
 * itself, which is written by closing the quotes, escaping it, and reopening.
 */
fun String.shellQuoted(): String = "'" + replace("'", "'\\''") + "'"

/**
 * Where the module's files live once it is installed. KernelSU, Magisk and
 * APatch use the same layout under /data/adb.
 */
object ModulePaths {
    const val MODULE_ID = "adblocker"
    const val MODULE_DIR = "/data/adb/modules/$MODULE_ID"
    const val ABD = "$MODULE_DIR/bin/abd"
    const val MODULE_PROP = "$MODULE_DIR/module.prop"

    /** Where a module update is staged until the next boot. */
    const val PENDING_UPDATE_DIR = "/data/adb/modules_update/$MODULE_ID"

    /** The daemon's data: lists, rules, the authority's key. Root only. */
    const val DATA_DIR = "/data/adb/$MODULE_ID"
    const val LOG = "$DATA_DIR/abd.log"
}
