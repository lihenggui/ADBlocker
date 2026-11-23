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

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PowerManager
import android.os.Process
import android.provider.Settings
import androidx.core.content.edit
import androidx.core.net.toUri
import dagger.hilt.android.qualifiers.ApplicationContext
import li.mercury.adblocker.core.model.VpnState
import javax.inject.Inject
import javax.inject.Singleton

/** Persists only enough information to recognize the loss of an established VPN. */
@Singleton
internal class VpnBatteryRecovery @Inject constructor(@ApplicationContext private val context: Context) {
    private val prefs by lazy { context.getSharedPreferences("vpn_recovery", Context.MODE_PRIVATE) }

    fun established() {
        if (prefs.getInt(PID, 0) == Process.myPid()) return
        prefs.edit {
            putInt(PID, Process.myPid())
            putLong(SINCE, System.currentTimeMillis())
            remove(INTERRUPTED)
        }
    }

    fun interrupted() {
        if (prefs.getInt(PID, 0) == Process.myPid()) prefs.edit { putBoolean(INTERRUPTED, true) }
    }

    fun clear() = prefs.edit { clear() }

    fun shouldShow(state: VpnState, prepared: Boolean): Boolean = shouldOfferBatteryExemption(state, prepared, ::clear) {
        val pid = prefs.getInt(PID, 0)
        if (pid == 0) return@shouldOfferBatteryExemption false
        val exempt = context.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(context.packageName)
        if (exempt) {
            clear()
            return@shouldOfferBatteryExemption false
        }
        val interrupted = prefs.getBoolean(INTERRUPTED, false) && pid == Process.myPid()
        val killed = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && pid != Process.myPid()) {
            val since = prefs.getLong(SINCE, Long.MAX_VALUE)
            context.getSystemService(ActivityManager::class.java).getHistoricalProcessExitReasons(context.packageName, pid, 1)
                .firstOrNull()?.let { exit ->
                    exit.timestamp >= since && exit.reason == ApplicationExitInfo.REASON_LOW_MEMORY
                } == true
        } else {
            false
        }
        interrupted || killed
    }

    /**
     * The system's request for this app, or its battery optimization list
     * where a device has no request screen. Null when it has neither, so
     * the notice never launches nothing.
     */
    fun requestIntent(): Intent? = listOf(
        Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, "package:${context.packageName}".toUri()),
        Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS),
    ).firstOrNull { it.resolveActivity(context.packageManager) != null }

    private companion object {
        const val PID = "vpn_pid"
        const val SINCE = "vpn_since"
        const val INTERRUPTED = "vpn_interrupted"
    }
}

internal fun shouldOfferBatteryExemption(
    state: VpnState,
    prepared: Boolean,
    clear: () -> Unit,
    observedStop: () -> Boolean,
): Boolean {
    if (!prepared || state == VpnState.Revoked || state == VpnState.Failed) {
        clear()
        return false
    }
    return state == VpnState.Off && observedStop()
}
