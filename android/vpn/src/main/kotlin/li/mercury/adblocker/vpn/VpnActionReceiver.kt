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

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.components.SingletonComponent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.DaemonRepository
import timber.log.Timber

/**
 * The actions of the VPN's notification. A broadcast rather than an intent
 * to the service: a start of the service is a start of the VPN. Pausing
 * leaves the VPN as it is; the engine hands the connections on unread.
 */
class VpnActionReceiver : BroadcastReceiver() {
    @EntryPoint
    @InstallIn(SingletonComponent::class)
    internal interface Dependencies {
        fun daemon(): DaemonRepository

        @ApplicationScope
        fun scope(): CoroutineScope
    }

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action
        if (action != ACTION_PAUSE && action != ACTION_RESUME) return
        val dependencies = EntryPointAccessors.fromApplication(context, Dependencies::class.java)
        val daemon = dependencies.daemon()
        val pending = goAsync()
        dependencies.scope().launch {
            try {
                suspendRunCatching {
                    if (action == ACTION_PAUSE) daemon.pause(PAUSE_MINUTES) else daemon.resume()
                }.onFailure { Timber.w("vpn: notification action: %s", it.javaClass.simpleName) }
            } finally {
                pending.finish()
            }
        }
    }

    internal companion object {
        const val ACTION_PAUSE = "li.mercury.adblocker.vpn.PAUSE"
        const val ACTION_RESUME = "li.mercury.adblocker.vpn.RESUME"
        const val PAUSE_MINUTES = 60
    }
}
