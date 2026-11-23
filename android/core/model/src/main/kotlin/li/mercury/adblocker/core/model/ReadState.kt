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

package li.mercury.adblocker.core.model

/** A read whose result can be shown independently of the rest of the screen. */
sealed interface ReadState<out T> {
    data object Pending : ReadState<Nothing>

    data class Available<T>(val value: T) : ReadState<T>

    data class Unavailable(val failure: LoadFailure) : ReadState<Nothing>

    val valueOrNull: T?
        get() = when (this) {
            is Available -> value
            Pending, is Unavailable -> null
        }

    fun <R> map(transform: (T) -> R): ReadState<R> = when (this) {
        is Available -> Available(transform(value))
        is Unavailable -> this
        Pending -> Pending
    }
}
