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

package li.mercury.adblocker.core.navigation

import androidx.navigation3.runtime.NavBackStack
import androidx.navigation3.runtime.NavKey
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class NavigatorTest {
    private data object Home : NavKey

    private data object Lists : NavKey

    private data object Detail : NavKey

    private fun navigator(): Navigator {
        val state = NavigationState(
            startKey = Home,
            topLevelStack = NavBackStack(Home),
            subStacks = mapOf(Home to NavBackStack(Home), Lists to NavBackStack(Lists)),
        )
        return Navigator(state)
    }

    @Test
    fun givenADetailOpen_whenAnotherDestinationIsChosen_thenTheMoveIsBetweenTopLevels() {
        val navigator = navigator()
        navigator.navigate(Detail)
        navigator.navigate(Lists)
        assertTrue(navigator.movedBetweenTopLevels)
        assertEquals(Lists, navigator.state.currentTopLevelKey)
    }

    @Test
    fun givenADestination_whenAScreenAboveItOpens_thenTheMoveIsNotBetweenTopLevels() {
        val navigator = navigator()
        navigator.navigate(Lists)
        navigator.navigate(Detail)
        assertFalse(navigator.movedBetweenTopLevels)
    }

    @Test
    fun givenASecondDestination_whenGoingBack_thenTheMoveIsBetweenTopLevelsAndPreviewedSo() {
        val navigator = navigator()
        navigator.navigate(Lists)
        assertTrue(navigator.backLeavesTopLevel)
        navigator.goBack()
        assertTrue(navigator.movedBetweenTopLevels)
        assertEquals(Home, navigator.state.currentKey)
    }

    @Test
    fun givenAScreenAboveADestination_whenGoingBack_thenItStaysOnTheDestination() {
        val navigator = navigator()
        navigator.navigate(Lists)
        navigator.navigate(Detail)
        assertFalse(navigator.backLeavesTopLevel)
        navigator.goBack()
        assertFalse(navigator.movedBetweenTopLevels)
        assertEquals(Lists, navigator.state.currentKey)
    }

    @Test
    fun givenAScreenAboveADestination_whenTheDestinationIsChosenAgain_thenItReturnsToItsRoot() {
        val navigator = navigator()
        navigator.navigate(Lists)
        navigator.navigate(Detail)
        navigator.navigate(Lists)
        assertFalse(navigator.movedBetweenTopLevels)
        assertEquals(Lists, navigator.state.currentKey)
    }
}
