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

package li.mercury.adblocker.core.engine

import timber.log.Timber
import kotlin.test.Test
import kotlin.test.assertEquals

class ConnectionOwnersTest {
    @Test
    fun givenAndroidCannotIdentifyConnections_whenLookupsFailRepeatedly_thenTheServiceIsCachedAndWarningsAreThrottled() {
        val messages = mutableListOf<String>()
        val tree = object : Timber.Tree() {
            override fun log(priority: Int, tag: String?, message: String, t: Throwable?) {
                messages += message
            }
        }
        var serviceLookups = 0
        var elapsed = 0L
        val owners = ConnectionOwners({
            serviceLookups++
            null
        }, { elapsed })
        val local = byteArrayOf(10, 0, 0, 5)
        val remote = byteArrayOf(10, 0, 0, 6)
        Timber.plant(tree)
        try {
            repeat(50) { assertEquals(-1, owners.lookup(6, local, 32000, remote, 443)) }
            assertEquals(1, serviceLookups)
            assertEquals(listOf("owner uid: IllegalStateException"), messages)
            elapsed = 60_000L
            assertEquals(-1, owners.lookup(6, local, 32000, remote, 443))
            assertEquals(2, messages.size)
        } finally {
            Timber.uproot(tree)
        }
    }
}
