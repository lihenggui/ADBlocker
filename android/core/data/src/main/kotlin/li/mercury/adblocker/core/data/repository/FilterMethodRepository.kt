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

import li.mercury.adblocker.core.model.FilterMethod

enum class FilterMethodSwitch {
    Complete,
    RootPending,
    VpnTransferIncomplete,
}

/** Choosing between the root module and the VPN. */
interface FilterMethodRepository {
    /** Whether root is granted, so that the root module can be chosen. */
    suspend fun rootAvailable(): Boolean

    /**
     * Makes [method] the one in use and carries over the configuration.
     * Root can remain pending. VPN can be selected even if some settings could not be carried.
     */
    suspend fun switchTo(method: FilterMethod): FilterMethodSwitch

    /** Retries a requested switch or cancellation. True once the switch completes. */
    suspend fun retryRootSwitch(): Boolean

    /** Cancels a requested switch before the active method changes. */
    suspend fun cancelRootSwitch()
}
