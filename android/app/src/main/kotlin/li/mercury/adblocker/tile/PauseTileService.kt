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

package li.mercury.adblocker.tile

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.TileService
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.distinctUntilChangedBy
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import li.mercury.adblocker.MainActivity
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.VpnState
import timber.log.Timber
import javax.inject.Inject

/**
 * Pauses and resumes filtering from Quick Settings: the quickest way out
 * when a filtered app misbehaves. Paused from here, filtering stays paused
 * until resumed, as a switch in Quick Settings stays where it was put.
 * With the VPN method, a VPN that is off is started from here too.
 */
@AndroidEntryPoint
class PauseTileService : TileService() {
    @Inject
    lateinit var daemon: DaemonRepository

    @Inject
    lateinit var userData: UserDataRepository

    @Inject
    lateinit var vpn: VpnController

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var listening: Job? = null
    private var view: TileView? = null

    override fun onStartListening() {
        super.onStartListening()
        listening?.cancel()
        // Followed while the panel shows the tile: a VPN started from it
        // is shown starting, then on.
        listening = scope.launch {
            vpn.status.distinctUntilChangedBy { it.state }.collect { show(load()) }
        }
    }

    override fun onStopListening() {
        listening?.cancel()
        super.onStopListening()
    }

    override fun onClick() {
        super.onClick()
        when (view?.click ?: return) {
            TileClick.None -> Unit

            TileClick.Pause -> act { daemon.pause(minutes = null) }

            TileClick.Resume -> act { daemon.resume() }

            TileClick.StartVpn -> scope.launch {
                // Started or not, the app says why: no engine, or consent taken back.
                suspendRunCatching { vpn.start() }.onFailure {
                    Timber.w("tile: vpn start: %s", it.javaClass.simpleName)
                    openApp()
                }
            }

            TileClick.OpenApp -> openApp()
        }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun act(change: suspend () -> Unit) {
        scope.launch {
            suspendRunCatching { change() }.onFailure { Timber.w("tile: %s", it.javaClass.simpleName) }
            show(load())
        }
    }

    private suspend fun load(): TileView {
        val method = userData.userData.first().method
        val state = vpn.status.value.state
        val status = if (method == FilterMethod.Root || state == VpnState.On) status() else null
        return tileView(
            method = method,
            daemon = status,
            vpn = state,
            // Loading the library reads the disk: not on the main thread.
            engineAvailable = method == FilterMethod.Vpn && vpn.engineAvailable(),
            consentGiven = method == FilterMethod.Vpn && vpn.consentIntent() == null,
        )
    }

    private suspend fun status(): DaemonStatus? = suspendRunCatching { daemon.status() }
        .onFailure { Timber.w("tile status: %s", it.javaClass.simpleName) }
        .getOrNull()

    private fun show(loaded: TileView) {
        view = loaded
        val tile = qsTile ?: return
        tile.state = loaded.state
        tile.subtitle = getString(loaded.subtitle)
        tile.updateTile()
    }

    // The intent is given only before Android 14, where it is the one way to collapse the panel.
    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun openApp() {
        val intent = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val open = {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
            } else {
                @Suppress("DEPRECATION")
                startActivityAndCollapse(intent)
            }
        }
        if (isLocked) unlockAndRun(open) else open()
    }
}
