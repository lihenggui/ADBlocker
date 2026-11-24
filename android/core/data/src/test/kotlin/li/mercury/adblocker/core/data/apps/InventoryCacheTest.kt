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

package li.mercury.adblocker.core.data.apps

import kotlinx.coroutines.async
import kotlinx.coroutines.test.runTest
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertSame

class InventoryCacheTest {
    @Test
    fun givenSeveralReaders_whenThePackageGenerationChanges_thenTheyShareOneFreshInventory() = runTest {
        val cache = InventoryCache<Long, List<String>>()
        var generation = 1L
        var loads = 0
        suspend fun read(): List<String> = cache.read({ generation }) { listOf("load ${++loads}") }

        val first = async { read() }
        val second = async { read() }
        assertSame(first.await(), second.await())
        assertEquals(1, loads)
        generation++
        assertEquals(listOf("load 2"), read())
        assertEquals(listOf("load 2"), read())
        assertEquals(2, loads)
    }

    @Test
    fun givenAFailedRead_whenAnotherCallerRetries_thenTheFailedResultIsNotCached() = runTest {
        val cache = InventoryCache<Int, String>()
        kotlin.test.assertFailsWith<IllegalStateException> { cache.read({ 1 }) { error("package manager unavailable") } }
        assertEquals("fresh", cache.read({ 1 }) { "fresh" })
    }
}
