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

import androidx.navigation3.runtime.NavKey

/*
 * Includes code from Blocker, modified for ADBlocker.
 * Copyright 2025 Blocker
 * Upstream license: Apache-2.0. See THIRD_PARTY_NOTICES.md.
 */

/**
 * Handles navigation events (forward and back) by updating [state].
 *
 * Follows the navigation design of Now in Android: each top-level
 * destination owns a back stack, and the stacks of the top-level
 * destinations visited form the overall back stack.
 */
class Navigator(val state: NavigationState) {

    /**
     * Whether the last move went from one top-level destination to another,
     * from wherever it was asked (the navigation bar, a link on a page, back):
     * such a move fades, as unrelated screens do in M3's top-level pattern,
     * rather than sliding. Read by the transitions, which run after the move.
     */
    var movedBetweenTopLevels: Boolean = false
        private set

    /** Whether going back now would leave this top-level destination for another, as a predictive back gesture previews. */
    val backLeavesTopLevel: Boolean
        get() = state.currentKey == state.currentTopLevelKey && state.currentKey != state.startKey

    /** Go to [key], which may be a top-level destination or a screen above one. */
    fun navigate(key: NavKey) {
        when (key) {
            // Re-selecting the current top-level destination returns to its root.
            state.currentTopLevelKey -> {
                movedBetweenTopLevels = false
                clearSubStack()
            }

            in state.topLevelKeys -> {
                movedBetweenTopLevels = true
                goToTopLevel(key)
            }

            else -> {
                movedBetweenTopLevels = false
                goToKey(key)
            }
        }
    }

    /**
     * Go back one screen. Does nothing on the start destination: a screen
     * that is animating out can still deliver a second "back".
     */
    fun goBack() {
        when (state.currentKey) {
            state.startKey -> Unit

            state.currentTopLevelKey -> {
                movedBetweenTopLevels = true
                state.topLevelStack.removeLastOrNull()
            }

            else -> {
                movedBetweenTopLevels = false
                state.currentSubStack.removeLastOrNull()
            }
        }
    }

    private fun goToKey(key: NavKey) {
        // Moving an already-open screen to the top avoids duplicates.
        state.currentSubStack.apply {
            remove(key)
            add(key)
        }
    }

    private fun goToTopLevel(key: NavKey) {
        state.topLevelStack.apply {
            if (key == state.startKey) clear() else remove(key)
            add(key)
        }
    }

    private fun clearSubStack() {
        state.currentSubStack.run {
            if (size > 1) subList(1, size).clear()
        }
    }
}
