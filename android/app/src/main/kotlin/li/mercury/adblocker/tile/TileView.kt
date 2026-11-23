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

import android.service.quicksettings.Tile
import androidx.annotation.StringRes
import li.mercury.adblocker.R
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.VpnState

/** What a tap on the tile does. */
internal enum class TileClick { None, Pause, Resume, StartVpn, OpenApp }

/** How the tile looks, a `Tile.STATE_*` and a subtitle, and what a tap does. */
internal data class TileView(val state: Int, @StringRes val subtitle: Int, val click: TileClick)

/**
 * The tile for the method in use. Filtering that runs is paused and
 * resumed, with either method. A VPN that is off is started by a tap once
 * the user has consented to it; before, the app is opened, where consent
 * is asked for, since the tile cannot ask.
 *
 * [daemon] is the status of the method in use; null when it could not be
 * read, or for a VPN that does not run.
 */
internal fun tileView(
    method: FilterMethod,
    daemon: DaemonStatus?,
    vpn: VpnState,
    engineAvailable: Boolean,
    consentGiven: Boolean,
): TileView = when (method) {
    FilterMethod.Root -> when {
        daemon?.running != true -> TileView(Tile.STATE_UNAVAILABLE, R.string.tile_stopped, TileClick.None)
        else -> filtering(daemon.paused)
    }

    FilterMethod.Vpn -> when {
        !engineAvailable -> TileView(Tile.STATE_UNAVAILABLE, R.string.tile_stopped, TileClick.None)

        // Not known to be paused: a tap pauses, the way out of filtering.
        vpn == VpnState.On -> filtering(daemon?.paused == true)

        vpn == VpnState.Starting -> TileView(Tile.STATE_ACTIVE, R.string.tile_filtering, TileClick.None)

        else -> TileView(Tile.STATE_INACTIVE, R.string.tile_stopped, if (consentGiven) TileClick.StartVpn else TileClick.OpenApp)
    }
}

private fun filtering(paused: Boolean) = if (paused) {
    TileView(Tile.STATE_INACTIVE, R.string.tile_paused, TileClick.Resume)
} else {
    TileView(Tile.STATE_ACTIVE, R.string.tile_filtering, TileClick.Pause)
}
