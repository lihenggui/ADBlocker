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

package li.mercury.adblocker.boot

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.core.data.repository.AutoStart
import javax.inject.Inject

/**
 * Has the daemon started once the device has booted, where no root manager
 * started it ([AutoStart]). Nothing is shown, and what the VPN method does
 * at boot is not this receiver's.
 */
@AndroidEntryPoint
class BootReceiver : BroadcastReceiver() {
    @Inject
    lateinit var autoStart: AutoStart

    @Inject
    @ApplicationScope
    lateinit var scope: CoroutineScope

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        // Off the main thread, with the broadcast held open so that the
        // process is kept meanwhile.
        val pending = goAsync()
        scope.launch {
            try {
                val start = launch { autoStart.afterBoot() }
                // A root command cannot be abandoned half way: past the
                // limit the start goes on by itself, and the broadcast is
                // not held until the system would end the process for it.
                withTimeoutOrNull(LIMIT_MS) { start.join() }
            } finally {
                pending.finish()
            }
        }
    }

    private companion object {
        /** Well within what the system allows a receiver of a background broadcast. */
        const val LIMIT_MS = 30_000L
    }
}
