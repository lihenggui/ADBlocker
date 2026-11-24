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

package li.mercury.adblocker.core.data

import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState

/** Why a call to the daemon or the root shell could not be completed. */
sealed class AbdException(message: String) : Exception(message) {
    /** The app has not been granted root. */
    class NoRoot : AbdException("root access not granted")

    /** The module, and with it abd, is not installed. */
    class ModuleMissing : AbdException("module not installed")

    /** This build carries no engine for filtering without root. */
    class EngineMissing : AbdException("no engine in this build")

    /** abd rejected a command or its arguments: an option it does not know, or a value that is not one. */
    open class CommandRejected(message: String = "invalid abd command or arguments") : AbdException(message)

    /**
     * The installed abd does not know a command this app uses: the module is
     * older than the app. A rejection like any other to whoever handles
     * those, and to the screens a reason to bring the module up to date.
     */
    class ModuleOutdated : CommandRejected("abd does not know the command")

    /** A root command failed; the message carries what it printed. */
    class CommandFailed(message: String) : AbdException(message)

    /** A root command took longer than it may, and was stopped. */
    class TimedOut : AbdException("command timed out")

    /**
     * abd printed something this app cannot read. The message names the
     * command and the kind of error only, never the text: an output can
     * carry hosts and addresses.
     */
    class BadOutput(message: String) : AbdException(message)
}

/** The user-facing reason behind an exception from a repository. */
fun Throwable.toLoadFailure(): LoadFailure = when (this) {
    is AbdException.NoRoot -> LoadFailure.NoRoot
    is AbdException.ModuleMissing -> LoadFailure.ModuleMissing
    is AbdException.ModuleOutdated -> LoadFailure.ModuleOutdated
    is AbdException.EngineMissing -> LoadFailure.EngineMissing
    is AbdException.TimedOut -> LoadFailure.TimedOut
    else -> LoadFailure.Failed
}

/** Reads an independent section without treating a failure as an empty result. */
suspend fun <T> readSection(read: suspend () -> T): ReadState<T> = suspendRunCatching { read() }.fold(
    onSuccess = { ReadState.Available(it) },
    onFailure = { ReadState.Unavailable(it.toLoadFailure()) },
)
