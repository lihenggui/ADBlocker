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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.testing.TestUserDataRepository
import java.io.IOException
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

class MissingListsCheckTest {
    private val userData = TestUserDataRepository(UserData(onboardingDone = true))
    private var version = 7L
    private var reads = 0
    private var updates = 0
    private var read: suspend () -> List<FilterList> = { listOf(list(enabled = true, updated = null)) }

    private val check = MissingListsCheck(
        versionCode = { version },
        userData = userData,
        readLists = {
            reads++
            read()
        },
        requestUpdate = { updates++ },
        readTimeout = 8.seconds,
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

    @Test
    fun givenANewVersion_whenTheCheckRuns_thenTheListsAreReadAndTheVersionIsStored() = runTest {
        check.run()

        assertEquals(1, reads)
        assertEquals(1, updates)
        assertEquals(7L, userData.listsCheckedVersion())
    }

    @Test
    fun givenTheVersionWasChecked_whenTheCheckRunsAgain_thenTheListsAreNotRead() = runTest {
        check.run()
        check.run()

        assertEquals(1, reads)
        assertEquals(1, updates)
    }

    @Test
    fun givenTheVersionChanged_whenTheCheckRuns_thenTheListsAreReadAgain() = runTest {
        check.run()
        version = 8L
        read = { listOf(list(enabled = true, updated = Instant.EPOCH)) }

        check.run()

        assertEquals(2, reads)
        assertEquals(1, updates)
        assertEquals(8L, userData.listsCheckedVersion())
    }

    @Test
    fun givenTheListsCannotBeRead_whenTheCheckRuns_thenTheVersionIsNotStoredAndTheNextRunReadsAgain() = runTest {
        read = { throw IOException("unreadable") }
        check.run()
        assertNull(userData.listsCheckedVersion())

        read = { emptyList() }
        check.run()

        assertEquals(2, reads)
        assertEquals(0, updates)
        assertEquals(7L, userData.listsCheckedVersion())
    }

    @Test
    fun givenReadingTheListsDoesNotEnd_whenTheCheckRuns_thenItGivesUpWithoutStoringTheVersion() = runTest {
        read = { awaitCancellation() }

        check.run()

        assertEquals(1, reads)
        assertNull(userData.listsCheckedVersion())
    }

    @Test
    fun givenTwoCallersAtOnce_whenTheCheckRuns_thenTheListsAreReadOnce() = runTest {
        val gate = CompletableDeferred<List<FilterList>>()
        read = { gate.await() }

        val first = launch { check.run() }
        val second = launch { check.run() }
        runCurrent()
        gate.complete(listOf(list(enabled = true, updated = null)))
        first.join()
        second.join()

        assertEquals(1, reads)
        assertEquals(1, updates)
    }

    @Test
    fun givenSetupNotDone_whenTheCheckRuns_thenNothingIsReadOrStored() = runTest {
        userData.setOnboardingDone(false)

        check.run()

        assertEquals(0, reads)
        assertNull(userData.listsCheckedVersion())
    }

    private fun list(enabled: Boolean, updated: Instant?) = FilterList(
        url = "https://lists.example/list.txt",
        enabled = enabled,
        title = null,
        rules = null,
        updated = updated,
        error = null,
    )
}
