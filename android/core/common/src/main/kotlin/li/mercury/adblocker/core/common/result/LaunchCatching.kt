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

package li.mercury.adblocker.core.common.result

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import timber.log.Timber

/** Report an operation failure without putting exception messages into the log. */
suspend inline fun <T> suspendRunCatching(operation: String, crossinline block: suspend () -> T): Result<T> = suspendRunCatching { block() }.onFailure { Timber.w("$operation: %s", it.javaClass.simpleName) }

inline fun CoroutineScope.launchCatching(
    operation: String,
    crossinline onFailure: suspend (Throwable) -> Unit = {},
    crossinline block: suspend CoroutineScope.() -> Unit,
): Job = launch {
    suspendRunCatching(operation) { block() }.onFailure { onFailure(it) }
}
