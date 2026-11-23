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

package li.mercury.adblocker.core.data.vpn

import android.content.Intent
import kotlinx.coroutines.flow.StateFlow
import li.mercury.adblocker.core.model.VpnStatus

/**
 * The VPN of the VPN method, as the screens see it. Implemented by the VPN
 * service's module; with the root method it stays off.
 */
interface VpnController {
    val status: StateFlow<VpnStatus>

    suspend fun engineAvailable(): Boolean

    /** The system's consent screen to show before [start], or null if consent was given. */
    fun consentIntent(): Intent?

    /** Starts the VPN for the chosen apps, after consent. */
    suspend fun start()

    suspend fun stop()

    fun shouldOfferBatteryExemption(): Boolean = false

    fun requestBatteryExemptionIntent(): Intent? = null

    fun dismissBatteryExemption() = Unit
}
