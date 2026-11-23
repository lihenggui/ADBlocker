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

import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.util.UUID
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull

class FileRuleDraftStoreTest {
    @get:Rule
    val temporary = TemporaryFolder()

    @Test
    fun givenAnExistingCheckpoint_whenReadAndDiscarded_thenOnlyItsSavedEntryCanRestoreIt() = runTest {
        val id = UUID.randomUUID()
        val file = temporary.newFile("$id.txt").apply { writeText("||previous.example^") }
        val interrupted = temporary.newFile("$id.tmp").apply { writeText("partial") }
        val dispatcher = StandardTestDispatcher(testScheduler)
        val store = FileRuleDraftStore(temporary.root, dispatcher)

        assertEquals("||previous.example^", store.read(id))
        assertFalse(interrupted.exists())
        assertNull(store.read(UUID.randomUUID()))

        store.write(id, "||updated.example^")
        assertEquals("||updated.example^", file.readText())
        val restarted = FileRuleDraftStore(temporary.root, dispatcher)
        assertEquals("||updated.example^", restarted.read(id))
        restarted.write(id, null)

        assertFalse(file.exists())
        assertNull(store.read(id))
    }

    @Test
    fun givenAnExistingCheckpoint_whenAnEditExceedsTheUtf8Bound_thenTheCheckpointSurvives() = runTest {
        val id = UUID.randomUUID()
        val store = FileRuleDraftStore(temporary.root, StandardTestDispatcher(testScheduler))
        store.write(id, "||previous.example^")

        assertFailsWith<RulesTooLargeException> { store.write(id, "é".repeat(MAX_RULES_BYTES / 2 + 1)) }
        assertEquals("||previous.example^", store.read(id))
    }
}
