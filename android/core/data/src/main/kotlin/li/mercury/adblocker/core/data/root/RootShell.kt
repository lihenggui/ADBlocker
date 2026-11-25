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
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
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
    private val children = RootChildProcesses()

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
            val process = startRootProcess { children.start("exec timeout $DEFAULT_TIMEOUT_SECONDS $command") }
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
        children.start(command)
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

/** Selects the mount option before starting a caller's command, which may change state. */
internal class RootChildProcesses(private val launch: (List<String>) -> Process = { ProcessBuilder(it).start() }) {
    private val mountMaster by lazy {
        val probe = launch(listOf("su", "--mount-master", "-c", "true"))
        try {
            probe.outputStream.close()
            if (!probe.waitFor(5, TimeUnit.SECONDS)) throw AbdException.TimedOut()
            probe.exitValue() == 0
        } finally {
            probe.destroyForcibly()
        }
    }

    fun start(command: String): Process {
        val prefix = if (mountMaster) listOf("su", "--mount-master", "-c") else listOf("su", "-c")
        return launch(prefix + command)
    }
}

private fun startRootProcess(start: () -> Process): Process = try {
    start()
} catch (_: IOException) {
    throw AbdException.NoRoot()
}

// A rules export can be as large as the 16 MiB rules file.
private const val MAX_STDOUT = 16 * 1024 * 1024
private const val MAX_STDERR = 16 * 1024
private const val STDERR_JOIN_MS = 1_000L
private const val PIPE_FAILURE_WAIT_MS = 1_000L

internal fun streamProcess(label: String, start: () -> Process): Flow<String> = callbackFlow {
    val process = startRootProcess(start)
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
        val stdout = async { process.inputStream.use { it.readCapped(MAX_STDOUT) } }
        val stderr = async { process.errorStream.use { it.readCapped(MAX_STDERR) } }
        val writeFailure = try {
            process.outputStream.bufferedWriter().use { it.write(input) }
            null
        } catch (e: IOException) {
            // A missing executable or a timeout can close stdin before the payload is written.
            e
        }
        val code = process.waitFor()
        if (code == 0 && writeFailure != null) throw writeFailure
        val output = stdout.await()
        val error = stderr.await()
        if (code == 0 && output.truncated) throw AbdException.BadOutput("Root command output exceeded the size limit")
        CommandResult(code == 0, code, output.text, error.text)
    } finally {
        process.destroy()
    }
}

private data class CapturedOutput(val text: String, val truncated: Boolean)

private fun InputStream.readCapped(limit: Int): CapturedOutput {
    val output = ByteArrayOutputStream()
    val buffer = ByteArray(8192)
    var truncated = false
    while (true) {
        val count = read(buffer)
        if (count < 0) break
        val kept = minOf(count, limit - output.size())
        output.write(buffer, 0, kept)
        // Drain the pipe after the limit so the child can finish and report its exit.
        if (kept < count) truncated = true
    }
    return CapturedOutput(output.toString(Charsets.UTF_8.name()), truncated)
}

/** What toybox `timeout` exits with when it stopped the command. */
internal const val TIMED_OUT = 124

/**
 * [command] in a shell of its own, stopped after [seconds]. Its standard input
 * is empty: the shared shell reads the next commands from its own, which a
 * command reading it would take.
 */
internal fun bounded(command: String, seconds: Int): String = "timeout $seconds sh -c ${command.shellQuoted()} </dev/null"

/** How abd begins what it says of a command it does not know (abd/src/main.rs, abd-engine/src/cli/mod.rs). */
private const val UNKNOWN_COMMAND = "abd: unknown command"

/** The exception for an abd command that exited with [code]. */
internal fun exitFailure(code: Int, output: String): AbdException = when (code) {
    // The shell could not find abd: no module.
    126, 127 -> AbdException.ModuleMissing()

    // abd's usage error. A command it does not know at all is one a newer
    // app asks of an older module; the rest of what it prints can name what
    // was asked about, and is not kept.
    2 -> if (output.trimStart().startsWith(UNKNOWN_COMMAND)) AbdException.ModuleOutdated() else AbdException.CommandRejected()

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
 * Where the module's files live once it is installed: in the directory
 * KernelSU, Magisk and APatch keep their modules in when it was flashed in
 * one of them, or in a directory of its own when this app installed it with
 * root alone (module/direct.sh).
 */
object ModulePaths {
    const val MODULE_ID = "adblocker"

    /** The module this app installed, which no root manager lists. */
    const val DIRECT_DIR = "/data/adb/${MODULE_ID}_direct"

    /**
     * Finds the module in use, for the rest of a shell command to name as
     * [MODULE_DIR]: the one in the root manager once it has its daemon, or
     * else the one this app installed. A manager keeps a directory without
     * the daemon for a module it applies at the next boot. Asked in every
     * command: a module is flashed or removed while the app runs.
     */
    const val LOCATE = "m=/data/adb/modules/$MODULE_ID; [ -e \$m/bin/abd ] || m=$DIRECT_DIR"

    /** The module's directory, in a command that begins with [LOCATE]. */
    const val MODULE_DIR = "\$m"

    /** The daemon's file, in a command that begins with [LOCATE]. */
    const val DAEMON = "$MODULE_DIR/bin/abd"
    const val MODULE_PROP = "$MODULE_DIR/module.prop"

    /**
     * `abd` of the module in use, as one command that takes its arguments
     * and its input: it also goes after `exec`, and into the background.
     */
    const val ABD = "sh -c '$LOCATE; exec $DAEMON \"\$@\"' abd"

    /** Where a module update is staged until the next boot. */
    const val PENDING_UPDATE_DIR = "/data/adb/modules_update/$MODULE_ID"

    /** The daemon's data: lists, rules, the authority's key. Root only. */
    const val DATA_DIR = "/data/adb/$MODULE_ID"
    const val LOG = "$DATA_DIR/abd.log"
}
