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

package li.mercury.adblocker.vpn

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.sync.Mutex
import li.mercury.adblocker.core.model.VpnStatus
import javax.inject.Inject
import javax.inject.Singleton

/** The VPN of this process, between its service and its controller. */
@Singleton
internal class RunningVpn @Inject constructor() {
    /**
     * Always on and lockdown are known only while the service runs: they
     * keep the values it last read, which say what the apps have once it
     * stopped.
     */
    val status = MutableStateFlow(VpnStatus())

    /**
     * Held for every step that touches the engine's stack, so that one
     * instance of the service has stopped it before the next starts it.
     */
    val lock = Mutex()

    /** The session of the service while it exists. */
    @Volatile
    var session: VpnSession? = null
}
