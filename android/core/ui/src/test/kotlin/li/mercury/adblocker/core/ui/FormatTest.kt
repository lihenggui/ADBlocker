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

package li.mercury.adblocker.core.ui

import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.time.Instant
import java.util.Locale
import kotlin.test.assertEquals

@RunWith(RobolectricTestRunner::class)
class FormatTest {
    private val now = Instant.parse("2026-11-19T15:00:00Z")

    @Test
    fun relativeTime_isLowerCaseForTheMiddleOfASentence() {
        assertEquals("yesterday", relativeTime(now.minusSeconds(26 * 3_600), now, Locale.US))
    }

    @Test
    fun relativeTime_namesHoursAndMinutesInShortForm() {
        assertEquals("5 hr. ago", relativeTime(now.minusSeconds(5 * 3_600), now, Locale.US))
        assertEquals("12 min. ago", relativeTime(now.minusSeconds(12 * 60), now, Locale.US))
    }

    @Test
    fun relativeTime_isInTheLocalesWords_withChineseDigitsSpacedAsTheAppsCopy() {
        assertEquals("5 小时前", relativeTime(now.minusSeconds(5 * 3_600), now, Locale.SIMPLIFIED_CHINESE))
    }
}
