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

package li.mercury.adblocker.core.data.transport

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.root.ModulePaths.ABD
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.exitFailure
import li.mercury.adblocker.core.data.root.shellQuoted
import li.mercury.adblocker.core.engine.EngineExit
import li.mercury.adblocker.core.engine.EngineHost
import li.mercury.adblocker.core.model.FilterMethod
import java.io.IOException
import javax.inject.Inject
import javax.inject.Singleton

/**
 * How `abd` is reached: the root module's binary through a root shell, or
 * the engine in this process. Both run the same dispatcher and print the
 * same output (testdata/protocol), so the client above them is one.
 */
interface AbdTransport {
    /** `abd <args>`, with [input] on its standard input; its output, or an [AbdException]. */
    suspend fun run(args: List<String>, label: String, input: String? = null): String

    /**
     * `abd <args>`, a line at a time. Stopping the collection stops the
     * command; [endsWithInput] says whether the command ends by itself when
     * its input closes, as `watch` does and `update` does not. Cancellation
     * abandons any remaining progress lines. An interrupted read while
     * collecting fails the flow, even when the daemon reports success.
     */
    fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String>
}

/** The module's `abd`, run as root. */
@Singleton
class RootTransport @Inject constructor(private val shell: RootShell) : AbdTransport {
    override suspend fun run(args: List<String>, label: String, input: String?): String {
        val command = commandLine(args)
        val result = if (input == null && args != listOf("rules", "get")) {
            shell.run(command, label = "abd $label")
        } else {
            // The shared shell returns lines and drops the last newline. A rule file
            // must round-trip exactly through the editor and a method transfer.
            shell.runWithInput(command, label = "abd $label", input = input.orEmpty())
        }
        if (!result.isSuccess) throw exitFailure(result.code, result.error.ifBlank { result.output })
        return result.output
    }

    override fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String> {
        val command = commandLine(args)
        return shell.stream(if (endsWithInput) command else endingWithInput(command), "abd $label").catch { e ->
            if (e is IOException) throw AbdException.BadOutput("Could not read abd $label output").apply { initCause(e) }
            throw e
        }
    }

    private fun commandLine(args: List<String>) = (listOf(ABD) + args.map { it.shellQuoted() }).joinToString(" ")
}

/** The engine in this process, for filtering without root. */
@Singleton
class EngineTransport @Inject constructor(private val engine: EngineHost) : AbdTransport {
    override suspend fun run(args: List<String>, label: String, input: String?): String {
        if (!engine.available()) throw AbdException.EngineMissing()
        return try {
            engine.command(args, input?.toByteArray())
        } catch (e: EngineExit) {
            throw engineFailure(e)
        }
    }

    override fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String> = flow {
        if (!engine.available()) throw AbdException.EngineMissing()
        emitAll(engine.stream(args).catch { e -> throw if (e is EngineExit) engineFailure(e) else e })
    }

    private fun engineFailure(e: EngineExit) = exitFailure(e.exit, e.stderr)
}

/** Where commands go. */
interface TransportSource {
    /** The transport of the method in use. */
    suspend fun current(): AbdTransport

    fun observe(): Flow<AbdTransport> = flow { emit(current()) }
}

/** The transport of the method in use, as the settings say. */
@Singleton
class AbdTransports @Inject constructor(
    private val root: RootTransport,
    private val engine: EngineTransport,
    private val userData: UserDataRepository,
) : TransportSource {
    override suspend fun current(): AbdTransport = forMethod(userData.userData.first().method)

    override fun observe(): Flow<AbdTransport> = userData.userData.map { forMethod(it.method) }.distinctUntilChanged()

    private fun forMethod(method: FilterMethod): AbdTransport = when (method) {
        FilterMethod.Root -> root
        FilterMethod.Vpn -> engine
    }
}

/**
 * [command], run so that it ends when its standard input does, as
 * `abd watch` does by itself. `abd update` goes on to its end when nobody
 * reads what it prints, and two at once would replace each other's files:
 * a stopped collection must stop it. The watcher of the input is ended with
 * the command, so that it never signals a number another process was given.
 * Background jobs read /dev/null by default in a non-interactive shell, so
 * the watcher receives its own copy of stdin. Both jobs are reaped.
 */
internal fun endingWithInput(command: String): String = "(exec 3<&0; $command </dev/null 3<&- & p=\$!; " +
    "{ while IFS= read -r line; do :; done; kill \$p 2>/dev/null; } <&3 3<&- & w=\$!; " +
    "exec 3<&-; wait \$p; s=\$?; kill \$w 2>/dev/null; wait \$w 2>/dev/null; exit \$s)"
