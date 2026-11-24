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

package li.mercury.adblocker.sync.work

import li.mercury.adblocker.core.model.FilterList
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class UpdateSchedulerTest {
    private fun list(enabled: Boolean, updated: Instant?) = FilterList(
        url = "https://lists.example/list.txt",
        enabled = enabled,
        title = null,
        rules = null,
        updated = updated,
        error = null,
    )

    @Test
    fun givenAnEnabledListWithoutACopy_whenChecked_thenAnUpdateIsWanted() {
        val downloaded = list(enabled = true, updated = Instant.EPOCH)
        assertTrue(anyNeverDownloaded(listOf(downloaded, list(enabled = true, updated = null))))
    }

    @Test
    fun givenOnlyDisabledListsWithoutACopy_whenChecked_thenNoUpdateIsWanted() {
        val downloaded = list(enabled = true, updated = Instant.EPOCH)
        assertFalse(anyNeverDownloaded(listOf(downloaded, list(enabled = false, updated = null))))
        assertFalse(anyNeverDownloaded(emptyList()))
    }
}
