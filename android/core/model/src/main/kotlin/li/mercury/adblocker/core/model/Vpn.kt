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

/** Where the VPN of the VPN method stands. */
enum class VpnState {
    Off,
    Starting,
    On,

    /** Another VPN app, or the user in the system settings, ended the VPN; the chosen apps connect without filtering. */
    Revoked,

    /** The engine could not start or stopped by itself; the chosen apps connect without filtering. */
    Failed,
}

/**
 * The VPN and the system settings around it. [alwaysOn] and [lockdown] are
 * the system's "Always-on VPN" and "Block connections without VPN" for
 * this app: with the second, the chosen apps have no network while the
 * VPN is down.
 */
data class VpnStatus(
    val state: VpnState = VpnState.Off,
    val alwaysOn: Boolean = false,
    val lockdown: Boolean = false,
)
