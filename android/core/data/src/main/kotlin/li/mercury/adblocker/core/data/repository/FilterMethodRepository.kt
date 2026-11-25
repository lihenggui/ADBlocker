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
import li.mercury.adblocker.core.model.VpnTransferPart

sealed interface FilterMethodSwitch {
    data object Complete : FilterMethodSwitch
    data object RootPending : FilterMethodSwitch

    /** An empty set means the root configuration could not be read as a whole. */
    data class VpnTransferIncomplete(val failed: Set<VpnTransferPart> = emptySet()) : FilterMethodSwitch
}

/** Choosing between the root module and the VPN. */
interface FilterMethodRepository {
    /** Whether root is granted, so that the root module can be chosen. */
    suspend fun rootAvailable(): Boolean

    /** Whether this process already knows root was granted, without opening a shell. */
    fun rootGrantKnown(): Boolean

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
