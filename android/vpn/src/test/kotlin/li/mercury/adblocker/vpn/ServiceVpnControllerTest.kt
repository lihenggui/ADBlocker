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

import android.content.ContextWrapper
import android.content.Intent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.engine.PersonalAppIdentity
import li.mercury.adblocker.core.engine.VpnBinding
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class ServiceVpnControllerTest {
    private class Engine(private val isAvailable: Boolean) : VpnEngine {
        override suspend fun available() = isAvailable

        override var binding: VpnBinding? = null

        override suspend fun start(tunFd: Int, ipv6: Boolean, dnsServers: List<String>): Int = error("not started by the controller")

        override suspend fun <T> withPersonalAppSnapshot(apps: List<PersonalAppIdentity>, refresh: suspend () -> T): T = error("snapshot not used by the controller")

        override suspend fun stop() = error("not stopped by the controller")
    }

    private val running = RunningVpn()

    private fun controller(available: Boolean = true, consent: Intent? = null) = ServiceVpnController(ContextWrapper(null), Engine(available), running) { consent }

    @Test
    fun withoutTheEngineTheVpnCannotStart() = runTest {
        assertFailsWith<AbdException.EngineMissing> { controller(available = false).start() }
        assertEquals(VpnState.Off, running.status.value.state)
    }

    @Test
    fun beforeConsentTheVpnCannotStart() = runTest {
        assertFailsWith<IllegalStateException> { controller(consent = Intent()).start() }
        assertEquals(VpnState.Off, running.status.value.state)
    }

    @Test
    fun stoppingAVpnThatFailedShowsItOffAndKeepsWhatTheSystemSaid() = runTest {
        running.status.value = VpnStatus(VpnState.Failed, alwaysOn = true, lockdown = true)
        controller().stop()
        assertEquals(VpnStatus(VpnState.Off, alwaysOn = true, lockdown = true), running.status.value)
    }
}
