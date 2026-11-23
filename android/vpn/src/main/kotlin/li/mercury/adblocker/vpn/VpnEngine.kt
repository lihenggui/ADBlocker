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

import li.mercury.adblocker.core.engine.EngineHost
import li.mercury.adblocker.core.engine.PersonalAppIdentity
import li.mercury.adblocker.core.engine.VpnBinding
import javax.inject.Inject

/**
 * The engine's stack, as the VPN drives it (docs/engine-jni.md). An
 * interface, so that the tests of the VPN stand in for the engine.
 */
internal interface VpnEngine {
    /** Whether this build carries the engine. */
    suspend fun available(): Boolean

    /** The VPN the engine protects its sockets with, and tells when its stack ends by itself. */
    var binding: VpnBinding?

    /**
     * Starts the stack on [tunFd]. The engine takes the descriptor once
     * called, and closes it on [stop]; 0 when the stack started.
     */
    suspend fun start(tunFd: Int, ipv6: Boolean, dnsServers: List<String>): Int

    suspend fun <T> withPersonalAppSnapshot(apps: List<PersonalAppIdentity>, refresh: suspend () -> T): T

    /** Stops the stack and closes its descriptor; returns once both are done. Nothing if none runs. */
    suspend fun stop()
}

internal class HostEngine @Inject constructor(private val host: EngineHost) : VpnEngine {
    override suspend fun available(): Boolean = host.available()

    override var binding: VpnBinding?
        get() = host.vpn
        set(value) {
            host.vpn = value
        }

    override suspend fun start(tunFd: Int, ipv6: Boolean, dnsServers: List<String>): Int = host.start(tunFd, ipv6, dnsServers)

    override suspend fun <T> withPersonalAppSnapshot(apps: List<PersonalAppIdentity>, refresh: suspend () -> T): T = host.withPersonalAppSnapshot(apps, refresh)

    override suspend fun stop() = host.stop()
}
