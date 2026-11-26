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

package li.mercury.adblocker.core.designsystem.motion

import kotlin.test.Test
import kotlin.test.assertEquals

class MotionTest {
    @Test
    fun givenLtr_whenSlidingBetweenDestinations_thenForwardEntersFromTheRight() {
        assertEquals(96, enterOffset(96, forward = true, isRtl = false))
        assertEquals(-96, enterOffset(96, forward = false, isRtl = false))
    }

    @Test
    fun givenRtl_whenSlidingBetweenDestinations_thenForwardEntersFromTheLeft() {
        assertEquals(-96, enterOffset(96, forward = true, isRtl = true))
        assertEquals(96, enterOffset(96, forward = false, isRtl = true))
    }
}
