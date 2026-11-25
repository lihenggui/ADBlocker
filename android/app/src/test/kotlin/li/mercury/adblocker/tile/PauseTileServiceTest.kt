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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class PauseTileServiceTest {
    @Test
    fun givenTheTileIsLocked_whenClicked_thenFilteringChangesOnlyAfterUnlock() = runTest {
        var current = TileClick.Pause
        val actions = mutableListOf<TileClick>()
        val unlocks = mutableListOf<() -> Unit>()
        val tile = TileClicks(backgroundScope, { current }, { true }, { unlocks += it }) {
            actions += it
            current = TileClick.Resume
        }

        tile.click()
        tile.click()
        runCurrent()
        assertTrue(actions.isEmpty())

        unlocks.first()()
        runCurrent()
        unlocks.last()()
        runCurrent()

        assertEquals(listOf(TileClick.Pause), actions)
    }

    @Test
    fun givenAPauseCommandIsInFlight_whenTheTileIsTappedAgain_thenNoOtherCommandStartsUntilItFinishes() = runTest {
        var current = TileClick.Pause
        val actions = mutableListOf<TileClick>()
        val gate = CompletableDeferred<Unit>()
        val tile = TileClicks(backgroundScope, { current }, { false }, { error("already unlocked") }) {
            actions += it
            gate.await()
        }

        tile.click()
        tile.click()
        runCurrent()
        current = TileClick.Resume
        tile.click()
        runCurrent()
        assertEquals(listOf(TileClick.Pause), actions)

        gate.complete(Unit)
        runCurrent()
        tile.click()
        runCurrent()

        assertEquals(listOf(TileClick.Pause, TileClick.Resume), actions)
    }
}
