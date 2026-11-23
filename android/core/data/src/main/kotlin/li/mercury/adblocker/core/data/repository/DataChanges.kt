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

import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.merge
import li.mercury.adblocker.core.data.apps.InstalledAppsSource
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Told whenever the app changes the daemon's state, so that every screen
 * showing that state reads it again: an app chosen on its page is shown as
 * filtered in the list behind it.
 */
@Singleton
class DataChanges private constructor(installed: Flow<Unit>?) {
    @Inject constructor(apps: InstalledAppsSource) : this(apps.changes)

    constructor() : this(null)

    private val flow = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    val changes: Flow<Unit> = installed?.let { merge(flow, it) } ?: flow.asSharedFlow()

    fun notifyChanged() {
        flow.tryEmit(Unit)
    }

    /**
     * Runs [change], then tells of it whether it worked or not: one that
     * failed half way may have changed something all the same, and the
     * screens must show what the daemon has, not what was asked of it.
     */
    inline fun <T> changing(change: () -> T): T = try {
        change()
    } finally {
        notifyChanged()
    }
}
