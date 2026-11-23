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
import li.mercury.adblocker.R
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.VpnState
import kotlin.test.Test
import kotlin.test.assertEquals

class TileViewTest {
    private fun vpn(state: VpnState, daemon: DaemonStatus? = null, available: Boolean = true, consent: Boolean = true) = tileView(FilterMethod.Vpn, daemon, state, available, consent)

    private fun root(daemon: DaemonStatus?) = tileView(FilterMethod.Root, daemon, VpnState.Off, engineAvailable = false, consentGiven = false)

    @Test
    fun withTheRootModuleTheTilePausesAndResumesAsBefore() {
        assertEquals(TileView(Tile.STATE_ACTIVE, R.string.tile_filtering, TileClick.Pause), root(status(paused = false)))
        assertEquals(TileView(Tile.STATE_INACTIVE, R.string.tile_paused, TileClick.Resume), root(status(paused = true)))
        assertEquals(TileView(Tile.STATE_UNAVAILABLE, R.string.tile_stopped, TileClick.None), root(status(paused = false, running = false)))
        assertEquals(TileClick.None, root(null).click)
    }

    @Test
    fun aVpnThatIsOffIsStartedOnceConsentWasGiven() {
        for (state in listOf(VpnState.Off, VpnState.Revoked, VpnState.Failed)) {
            assertEquals(TileView(Tile.STATE_INACTIVE, R.string.tile_stopped, TileClick.StartVpn), vpn(state))
        }
    }

    @Test
    fun aVpnThatIsOffOpensTheAppBeforeConsent() {
        assertEquals(TileClick.OpenApp, vpn(VpnState.Off, consent = false).click)
    }

    @Test
    fun aVpnThatIsOnPausesAndResumes() {
        assertEquals(TileView(Tile.STATE_ACTIVE, R.string.tile_filtering, TileClick.Pause), vpn(VpnState.On, status(paused = false)))
        assertEquals(TileView(Tile.STATE_INACTIVE, R.string.tile_paused, TileClick.Resume), vpn(VpnState.On, status(paused = true)))
        // A pause that cannot be read is no reason to keep the user in.
        assertEquals(TileClick.Pause, vpn(VpnState.On, daemon = null).click)
    }

    @Test
    fun aVpnThatIsStartingIsNotStartedAgain() {
        assertEquals(TileClick.None, vpn(VpnState.Starting).click)
    }

    @Test
    fun withoutTheEngineTheVpnTileIsUnavailable() {
        assertEquals(TileView(Tile.STATE_UNAVAILABLE, R.string.tile_stopped, TileClick.None), vpn(VpnState.Off, available = false))
    }

    private fun status(paused: Boolean, running: Boolean = true) = DaemonStatus(
        running = running,
        pid = null,
        startedAt = null,
        version = null,
        apps = null,
        rules = null,
        hostRules = null,
        exclusions = null,
        paused = paused,
        resumeAt = null,
        ipv6 = null,
        listsUpdated = null,
    )
}
