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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class ListsRepositoryTest {
    @Test
    fun aRootUpdateCannotStartBeforeDeferredSubscriptionsAreSaved() = runTest {
        val settings = TestUserDataRepository(UserData(pendingDisabledLists = setOf("https://lists.example/disabled.txt")))
        val lists = AbdListsRepository(unexpected<AbdClient>(), unexpected<AbdClient>(), DataChanges(), settings)
        assertFailsWith<AbdException.CommandFailed> { lists.update().toList() }
        settings.setFilterMethod(FilterMethod.Vpn)
        assertFailsWith<AbdException.CommandFailed> { lists.updateRoot().toList() }
    }

    @Test
    fun twoUpdatesAskedAtOnceRunOneAfterTheOther() = runTest {
        var running = 0
        var mostAtOnce = 0
        val abd = object : AbdClient by unexpected() {
            override fun update(): Flow<UpdateEvent> = flow {
                running++
                mostAtOnce = maxOf(mostAtOnce, running)
                delay(1_000)
                emit(UpdateEvent.Finished(updated = 1, unchanged = 0, failed = 0))
                running--
            }
        }
        // As the Filters page and the background update would, on the one
        // instance the app has.
        val lists = AbdListsRepository(abd, abd, DataChanges(), TestUserDataRepository())
        val page = launch { lists.update().toList() }
        val background = launch { lists.updateRoot().toList() }
        testScheduler.advanceUntilIdle()
        page.join()
        background.join()
        assertEquals(1, mostAtOnce)
        assertEquals(0, running)
    }

    @Test
    fun givenAListWasFetchedByAnOngoingUpdate_whenItsQueuedDownloadRuns_thenItUsesTheLocalCopy() = runTest {
        val started = CompletableDeferred<Unit>()
        val finish = CompletableDeferred<Unit>()
        var saved = FilterList("https://lists.example/ads.txt", true, null, null, null, null)
        val abd = object : AbdClient by unexpected() {
            override suspend fun lists() = listOf(saved)

            override fun update(): Flow<UpdateEvent> = flow {
                started.complete(Unit)
                finish.await()
                saved = saved.copy(rules = 3)
            }
        }
        val lists = AbdListsRepository(abd, unexpected<AbdClient>(), DataChanges(), TestUserDataRepository())
        val all = launch { lists.update().toList() }
        started.await()
        val single = launch { assertTrue(lists.updateList(saved.url).toList().isEmpty()) }
        finish.complete(Unit)
        all.join()
        single.join()
    }

    @Test
    fun anUpdateThatNeverEndsTimesOut() = runTest {
        val abd = object : AbdClient by unexpected() {
            override fun update(): Flow<UpdateEvent> = flow { awaitCancellation() }
        }
        assertFailsWith<AbdException.TimedOut> { AbdListsRepository(abd, unexpected<AbdClient>(), DataChanges(), TestUserDataRepository()).update().toList() }
    }

    @Test
    fun anUpdateThatTimedOutLetsTheNextOneRun() = runTest {
        var runs = 0
        val abd = object : AbdClient by unexpected() {
            override fun update(): Flow<UpdateEvent> = flow {
                if (runs++ == 0) awaitCancellation()
                emit(UpdateEvent.Finished(updated = 0, unchanged = 1, failed = 0))
            }
        }
        val lists = AbdListsRepository(abd, unexpected<AbdClient>(), DataChanges(), TestUserDataRepository())
        assertFailsWith<AbdException.TimedOut> { lists.update().toList() }
        assertEquals(listOf(UpdateEvent.Finished(updated = 0, unchanged = 1, failed = 0)), lists.update().toList())
    }
}
