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

import android.content.Context
import android.content.Intent
import android.net.VpnService
import androidx.core.content.ContextCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import javax.inject.Inject
import javax.inject.Singleton

/** The VPN service, as the screens and the tile start and stop it. */
@Singleton
internal class ServiceVpnController internal constructor(
    private val context: Context,
    private val engine: VpnEngine,
    private val running: RunningVpn,
    private val recovery: VpnBatteryRecovery? = null,
    private val consent: () -> Intent?,
) : VpnController {
    @Inject
    constructor(
        @ApplicationContext context: Context,
        engine: VpnEngine,
        running: RunningVpn,
        recovery: VpnBatteryRecovery,
    ) : this(context, engine, running, recovery, { VpnService.prepare(context) })

    override val status: StateFlow<VpnStatus> = running.status.asStateFlow()

    override suspend fun engineAvailable(): Boolean = engine.available()

    override fun consentIntent(): Intent? = consent()

    /**
     * Asks the service to start, and returns: [status] goes on to On, or to
     * Failed or Revoked. Throws [AbdException.EngineMissing] without the
     * engine, and [IllegalStateException] before consent.
     */
    override suspend fun start() {
        if (!engineAvailable()) throw AbdException.EngineMissing()
        check(consent() == null) { "no consent for the VPN" }
        val before = running.status.value
        running.status.update { if (it.state == VpnState.On) it else it.copy(state = VpnState.Starting) }
        try {
            ContextCompat.startForegroundService(context, AdblockerVpnService.startIntent(context))
        } catch (e: RuntimeException) {
            // Not allowed to start, as from the background without consent.
            running.status.update { if (it.state == VpnState.Starting) before else it }
            throw e
        }
    }

    /** Stops the VPN, and returns once the engine has let go of it; Off also after a failure. */
    override suspend fun stop() {
        val session = running.session
        if (session != null) {
            session.stop()
        } else {
            recovery?.clear()
            running.status.update { it.copy(state = VpnState.Off) }
        }
    }

    override fun shouldOfferBatteryExemption() = recovery?.shouldShow(status.value.state, consent() == null) == true

    override fun requestBatteryExemptionIntent() = recovery?.requestIntent()

    override fun dismissBatteryExemption() {
        recovery?.clear()
    }
}
