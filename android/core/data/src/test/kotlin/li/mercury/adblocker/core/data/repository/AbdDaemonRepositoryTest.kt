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
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.testing.unexpected
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertSame
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class AbdDaemonRepositoryTest {
    private val status = File(checkNotNull(System.getProperty("protocolCases")), "status-filtering-apps.json").readText()
    private val commands = mutableListOf<List<String>>()
    private var runCommand: suspend (List<String>) -> String = { args ->
        if (args == listOf("status", "--json")) status else ""
    }
    private val transport = object : AbdTransport by unexpected() {
        override suspend fun run(args: List<String>, label: String, input: String?): String {
            commands += args
            return runCommand(args)
        }
    }
    private val changes = DataChanges()
    private val repository = AbdDaemonRepository(transport.fixedAbdClient(), changes)

    @Test
    fun givenDaemonStatus_whenRead_thenTheActiveCountIsReturnedWithoutSignallingAChange() = runTest {
        var notifications = 0
        backgroundScope.launch { changes.changes.collect { notifications++ } }
        runCurrent()

        val snapshot = repository.status()
        runCurrent()

        assertEquals(listOf(listOf("status", "--json")), commands)
        assertTrue(snapshot.running)
        assertEquals(5, snapshot.apps)
        assertEquals(3, snapshot.filteringApps)
        assertEquals(0, notifications)
    }

    @Test
    fun givenDaemonSettings_whenChanged_thenEveryCommandSignalsAReadback() = runTest {
        var notifications = 0
        backgroundScope.launch { changes.changes.collect { notifications++ } }
        runCurrent()

        repository.pause(null)
        runCurrent()
        repository.pause(15)
        runCurrent()
        repository.resume()
        runCurrent()
        repository.setUrlFiltering(false)
        runCurrent()
        repository.setCosmetic(true)
        runCurrent()
        repository.setFilteringScope(FilteringScope.All)
        runCurrent()

        assertEquals(
            listOf(
                listOf("pause"),
                listOf("pause", "--minutes", "15"),
                listOf("resume"),
                listOf("url-filtering", "off"),
                listOf("cosmetic", "on"),
                listOf("scope", "all"),
            ),
            commands,
        )
        assertEquals(commands.size, notifications)
    }

    @Test
    fun givenACommandFailsAfterStarting_whenTheCallFinishes_thenItSignalsReadbackAndPropagatesTheFailure() = runTest {
        val finish = CompletableDeferred<Unit>()
        val failure = AbdException.TimedOut()
        runCommand = {
            finish.await()
            throw failure
        }
        var notifications = 0
        backgroundScope.launch { changes.changes.collect { notifications++ } }
        runCurrent()

        val change = launch {
            assertSame(failure, assertFailsWith<AbdException.TimedOut> { repository.resume() })
        }
        runCurrent()
        assertEquals(listOf(listOf("resume")), commands)
        assertEquals(0, notifications)

        finish.complete(Unit)
        change.join()
        runCurrent()
        assertEquals(1, notifications)
    }
}
