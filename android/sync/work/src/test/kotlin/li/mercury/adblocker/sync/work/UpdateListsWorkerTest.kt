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

import androidx.work.ListenableWorker.Result
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class UpdateListsWorkerTest {
    private fun lists(update: () -> Flow<UpdateEvent>) = object : ListsRepository by unexpected() {
        override fun update(): Flow<UpdateEvent> = update()
    }

    private fun failing(e: Exception) = lists { flow { throw e } }

    private fun finished(updated: Int, unchanged: Int, failed: Int) = lists { flowOf(UpdateEvent.Started(3), UpdateEvent.Finished(updated, unchanged, failed)) }

    @Test
    fun anUpdateThatWorkedIsASuccess() = runTest {
        assertIs<Result.Success>(updateLists(finished(updated = 2, unchanged = 1, failed = 0)))
    }

    @Test
    fun aRootHandoffRefreshUsesTheRootLists() = runTest {
        val lists = object : ListsRepository by unexpected() {
            override fun updateRoot() = flowOf(UpdateEvent.Finished(updated = 2, unchanged = 0, failed = 0))
        }
        assertIs<Result.Success>(updateLists(lists, rootOnly = true))
    }

    @Test
    fun anUpdateWaitsForThePendingSubscriptionsAndRetriesAFailedWrite() = runTest {
        val actions = mutableListOf<String>()
        val lists = lists {
            actions += "download"
            flowOf(UpdateEvent.Finished(updated = 2, unchanged = 0, failed = 0))
        }

        assertIs<Result.Retry>(
            updateLists(lists) {
                actions += "save"
                throw AbdException.TimedOut()
            },
        )
        assertEquals(listOf("save"), actions)

        assertIs<Result.Success>(updateLists(lists) { actions += "save" })
        assertEquals(listOf("save", "save", "download"), actions)
    }

    @Test
    fun someListsFailingIsLeftForTheNextRun() = runTest {
        assertIs<Result.Success>(updateLists(finished(updated = 1, unchanged = 0, failed = 2)))
    }

    @Test
    fun anUpdateWithoutACompletionEventIsRetried() = runTest {
        assertIs<Result.Retry>(updateLists(lists { flowOf(UpdateEvent.Started(0)) }))
    }

    @Test
    fun missingHttpsExclusionsAreRetriedEvenWhenTheAdListsSucceeded() = runTest {
        val result = updateLists(lists { flowOf(UpdateEvent.Finished(1, 0, 0, exclusionsReady = false, exclusionsFailed = 1)) })
        assertIs<Result.Retry>(result)
    }

    @Test
    fun everyListFailingIsTriedAgainLater() = runTest {
        assertIs<Result.Retry>(updateLists(finished(updated = 0, unchanged = 0, failed = 3)))
    }

    @Test
    fun whatTheUserMustFixIsNotTriedAgain() = runTest {
        for (e in listOf(AbdException.NoRoot(), AbdException.ModuleMissing(), AbdException.CommandRejected(), AbdException.BadOutput("update: SerializationException"))) {
            assertEquals(Result.failure(), updateLists(failing(e)), e.javaClass.simpleName)
        }
    }

    @Test
    fun whatAnotherTryCanFixIsTriedAgain() = runTest {
        for (e in listOf(AbdException.CommandFailed("download failed"), AbdException.TimedOut())) {
            assertIs<Result.Retry>(updateLists(failing(e)), e.javaClass.simpleName)
        }
    }
}
