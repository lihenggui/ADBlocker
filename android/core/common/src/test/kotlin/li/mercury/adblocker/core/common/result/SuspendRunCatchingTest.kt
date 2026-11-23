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

package li.mercury.adblocker.core.common.result

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class SuspendRunCatchingTest {
    @Test
    fun aValueIsASuccess() = runTest {
        assertEquals(Result.success(1), suspendRunCatching { 1 })
    }

    @Test
    fun anExceptionIsAFailure() = runTest {
        assertTrue(suspendRunCatching { error("no") }.isFailure)
    }

    @Test
    fun aCancellationIsThrownOn() = runTest {
        assertFailsWith<CancellationException> { suspendRunCatching { throw CancellationException("gone") } }
    }
}
