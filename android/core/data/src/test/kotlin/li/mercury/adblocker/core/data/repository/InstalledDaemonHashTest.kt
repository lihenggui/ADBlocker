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

package li.mercury.adblocker.core.data.repository

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.RootShell
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class InstalledDaemonHashTest {
    private val first = "a".repeat(64)
    private val second = "b".repeat(64)
    private var answer = CommandResult(true, 0, "$first  /data/adb/modules/abd/bin/abd\n", "")
    private var hashed = 0
    private val hashes = InstalledDaemonHash(
        object : RootShell(Dispatchers.Unconfined) {
            override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
                hashed++
                return answer
            }
        },
    )

    @Test
    fun givenTheFileKeyIsUnchanged_whenHashedAgain_thenTheFileIsNotReadAgain() = runTest {
        assertEquals(first, hashes.hash("1 100 t z"))
        answer = CommandResult(true, 0, "$second  abd\n", "")
        assertEquals(first, hashes.hash("1 100 t z"))
        assertEquals(1, hashed)
    }

    @Test
    fun givenAModuleUpdateChangedTheFileKey_whenHashedAgain_thenItIsHashedAgain() = runTest {
        assertEquals(first, hashes.hash("1 100 t z"))
        answer = CommandResult(true, 0, "$second  abd\n", "")

        assertEquals(second, hashes.hash("2 100 t z"))
        assertEquals(2, hashed)
    }

    @Test
    fun givenNoFileKey_whenHashedAgain_thenItIsHashedEveryTime() = runTest {
        hashes.hash(null)
        hashes.hash(null)
        assertEquals(2, hashed)
    }

    @Test
    fun givenAnUnreadableFile_whenHashed_thenNothingIsKept() = runTest {
        answer = CommandResult(false, 1, "", "No such file")
        assertNull(hashes.hash("1 100 t z"))
        answer = CommandResult(true, 0, "$first  abd\n", "")

        assertEquals(first, hashes.hash("1 100 t z"))
        assertEquals(2, hashed)
    }

    @Test
    fun theFileKeyIsReadFromTheLineAfterTheVersion() {
        assertEquals("7 100 t z", daemonFileKey(listOf("version 12", "daemon-file 7 100 t z")))
        assertNull(daemonFileKey(listOf("version 12")))
    }
}
