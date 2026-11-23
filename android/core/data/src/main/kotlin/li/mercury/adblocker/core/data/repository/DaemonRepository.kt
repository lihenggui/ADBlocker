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

import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilteringScope
import javax.inject.Inject

/** Whether the daemon runs, and pausing it. */
interface DaemonRepository {
    suspend fun status(): DaemonStatus

    /** Pauses filtering of every app, for [minutes], or until resumed if null. */
    suspend fun pause(minutes: Int?)

    suspend fun resume()

    /** Off, no app is decrypted: every one is filtered by domain only. */
    suspend fun setUrlFiltering(on: Boolean)

    suspend fun setFilteringScope(scope: FilteringScope)

    /** On, the elements the lists name are hidden on decrypted pages. */
    suspend fun setCosmetic(on: Boolean)
}

internal class AbdDaemonRepository @Inject constructor(
    private val abd: AbdClient,
    private val changes: DataChanges,
) : DaemonRepository {
    override suspend fun status(): DaemonStatus = abd.status()

    override suspend fun setFilteringScope(scope: FilteringScope) = changes.changing { abd.setFilteringScope(scope) }

    override suspend fun setUrlFiltering(on: Boolean) = changes.changing { abd.setUrlFiltering(on) }

    override suspend fun setCosmetic(on: Boolean) = changes.changing { abd.setCosmetic(on) }

    override suspend fun pause(minutes: Int?) = changes.changing { abd.pause(minutes) }

    override suspend fun resume() = changes.changing { abd.resume() }
}
