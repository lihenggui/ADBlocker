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

package li.mercury.adblocker.hook

import java.lang.reflect.Modifier

/** What a check that failed returns instead, once it may pass. */
internal class Accepted(val result: Any?)

/**
 * Runs a certificate check as the app wrote it, and lets it pass only
 * where it failed and [accept] allows it.
 *
 * The check always runs first and whole: its result is returned as it is,
 * and a failure other than [isCheckFailure] is thrown on untouched. A
 * failure is thrown on unless [accept] returns what to return instead.
 * Should [accept] itself fail, in any way, the check's own failure stands.
 */
internal inline fun relaxed(
    check: () -> Any?,
    isCheckFailure: (Throwable) -> Boolean,
    accept: () -> Accepted?,
): Any? {
    val failure = try {
        return check()
    } catch (thrown: Throwable) {
        thrown
    }
    if (!isCheckFailure(failure)) throw failure
    val accepted = try {
        accept()
    } catch (_: Throwable) {
        // Whatever went wrong in deciding, the app's own result stands.
        null
    }
    return (accepted ?: throw failure).result
}

/**
 * The certificates an argument gives: a list, or a function of no
 * arguments that returns one, as OkHttp passes its cleaned chain. The
 * function is called through the single method of [type], the parameter's
 * declared interface, which keeps working where the app renamed it.
 */
internal fun certificatesOf(argument: Any?, type: Class<*>): List<*>? {
    if (argument is List<*>) return argument
    if (argument == null || !type.isInterface) return null
    val supplier = type.methods.singleOrNull {
        it.parameterCount == 0 && Modifier.isAbstract(it.modifiers)
    } ?: return null
    return supplier.invoke(argument) as? List<*>
}
