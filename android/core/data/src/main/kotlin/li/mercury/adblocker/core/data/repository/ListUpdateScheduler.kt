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

/**
 * The background list update, which sync:work carries out with WorkManager.
 * Features ask for an update through this, as Now in Android's SyncManager,
 * without depending on sync:work.
 */
interface ListUpdateScheduler {
    /**
     * Updates the lists once, as soon as there is a network the settings
     * allow: unmetered only when updates are set to Wi-Fi only.
     */
    suspend fun updateSoon()

    /** Refreshes the root module's lists after a handoff publishes or restores them. */
    suspend fun updateRootSoon()
}
