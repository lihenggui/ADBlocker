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

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.last
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.DefaultAbdClient
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.model.WatchEvent
import org.junit.Test
import java.lang.reflect.Proxy
import java.time.Instant
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class ActivityRepositoryTest {
    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun aMethodSwitchEndsTheOldWatchAndKeepsBothMethodsEvents() = runTest {
        val ended = mutableListOf<String>()
        fun transport(host: String): AbdTransport = object : AbdTransport {
            override suspend fun run(args: List<String>, label: String, input: String?) = error(label)

            override fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String> = flow {
                try {
                    emit("""{"time":1000,"app":"com.example.app","verdict":"blocked","host":"$host"}""")
                    awaitCancellation()
                } finally {
                    ended += host
                }
            }
        }
        val selectedTransport = MutableStateFlow(transport("vpn.example"))
        val abd = DefaultAbdClient(object : TransportSource {
            override suspend fun current() = selectedTransport.value

            override fun observe() = selectedTransport
        })
        var feed = ActivityFeed.Empty
        val recording = backgroundScope.launch { repository(abd).feed(null).collect { feed = it } }
        runCurrent()
        assertEquals(listOf("vpn.example"), feed.events.map { it.host })

        selectedTransport.value = transport("root.example")
        advanceTimeBy(ACTIVITY_SNAPSHOT_MS)
        runCurrent()
        assertEquals(listOf("vpn.example"), ended)
        assertEquals(listOf("root.example", "vpn.example"), feed.events.map { it.host })

        recording.cancel()
        runCurrent()
        assertEquals(listOf("vpn.example", "root.example"), ended)
    }

    private fun request(id: Long) = WatchEvent.Request(
        RequestEvent(id, Instant.ofEpochMilli(id), "com.example.news", Verdict.Blocked, "ads$id.example", null, null, null, null),
    )

    /** A client whose only command is `watch`, printing [events]. */
    private fun watching(events: List<WatchEvent>): AbdClient = watching { events.asFlow() }

    /** A client whose only command is `watch`, each run of it being [run]. */
    private fun watching(run: () -> Flow<WatchEvent>): AbdClient {
        val others = Proxy.newProxyInstance(AbdClient::class.java.classLoader, arrayOf(AbdClient::class.java)) { _, method, _ ->
            error("${method.name} is not expected")
        } as AbdClient
        return object : AbdClient by others {
            override fun watch(packageName: String?): Flow<WatchEvent> = run()
        }
    }

    private fun TestScope.repository(abd: AbdClient) = AbdActivityRepository(abd, StandardTestDispatcher(testScheduler))

    @OptIn(ExperimentalCoroutinesApi::class)
    @Test
    fun givenABusyWatch_whenSamplesArePublished_thenIntermediateEventsAreKeptWithoutPublishingEachOne() = runTest {
        val events = MutableSharedFlow<WatchEvent>()
        val snapshots = mutableListOf<ActivityFeed>()
        val buffer = ActivityBuffer()
        val recording = backgroundScope.launch {
            repository(watching { events }).feed(null, buffer).collect { snapshots += it }
        }
        runCurrent()
        events.emit(request(1))
        runCurrent()
        assertEquals(listOf(1L), snapshots.last().events.map { it.id })

        for (id in 2L..20L) events.emit(request(id))
        runCurrent()
        assertEquals(2, snapshots.size)
        advanceTimeBy(ACTIVITY_SNAPSHOT_MS)
        runCurrent()
        assertEquals(3, snapshots.size)
        assertEquals((20L downTo 1L).toList(), snapshots.last().events.map { it.id })

        events.emit(request(21))
        recording.cancel()
        runCurrent()
        assertEquals(20L, snapshots.last().events.first().id)
        assertEquals(21L, buffer.snapshot().events.first().id)
    }

    @Test
    fun givenMoreRequestsThanKept_whenWatched_thenOnlyTheNewestAreKeptNewestFirst() = runTest {
        val events = (1L..MAX_ACTIVITY_EVENTS + 20L).map(::request)
        val feed = repository(watching(events)).feed(null).last()
        assertEquals(MAX_ACTIVITY_EVENTS, feed.events.size)
        assertEquals(MAX_ACTIVITY_EVENTS + 20L, feed.events.first().id)
        assertEquals(21L, feed.events.last().id)
        assertEquals(20L, feed.dropped)
    }

    @Test
    fun givenTheDaemonDroppedEvents_whenWatched_thenTheCountsAddUp() = runTest {
        val events = listOf(request(1), WatchEvent.Dropped(12), request(2), WatchEvent.Dropped(3))
        val feed = repository(watching(events)).feed("com.example.news").last()
        assertEquals(15, feed.dropped)
        assertEquals(listOf(2L, 1L), feed.events.map { it.id })
    }

    @Test
    fun givenTheDaemonEndsTheWatch_whenWatched_thenItStartsAgainKeepingWhatWasSeen() = runTest {
        var runs = 0
        val client = watching {
            flow {
                runs++
                emit(request(runs.toLong()))
                if (runs < 3) throw AbdException.CommandFailed("the daemon ended the watch")
            }
        }
        val feed = repository(client).feed(null).last()
        assertEquals(3, runs)
        assertEquals(listOf(3L, 2L, 1L), feed.events.map { it.id })
    }

    @Test
    fun givenARejectedCommand_whenWatched_thenItIsNotTriedAgain() = runTest {
        var runs = 0
        val client = watching {
            flow<WatchEvent> {
                runs++
                throw AbdException.CommandRejected()
            }
        }
        assertFailsWith<AbdException.CommandRejected> { repository(client).feed(null).last() }
        assertEquals(1, runs)
    }

    @Test
    fun givenAWatchThatDeliversBeforeEachEnd_whenWatched_thenItIsStartedAgainMoreTimesThanTheRetries() = runTest {
        var runs = 0
        val client = watching {
            flow {
                runs++
                emit(request(runs.toLong()))
                if (runs <= WATCH_RETRIES * 2) throw AbdException.CommandFailed("the daemon ended the watch")
            }
        }
        val feed = repository(client).feed(null).last()
        assertEquals(WATCH_RETRIES * 2 + 1, runs)
        assertEquals(WATCH_RETRIES * 2 + 1, feed.events.size)
    }

    @Test
    fun givenAWatchThatEndsWithoutDelivering_whenWatched_thenItGivesUpAfterTheRetries() = runTest {
        var runs = 0
        val client = watching {
            flow<WatchEvent> {
                runs++
                throw AbdException.CommandFailed("the daemon is not running")
            }
        }
        assertFailsWith<AbdException.CommandFailed> { repository(client).feed(null).last() }
        assertEquals(WATCH_RETRIES + 1, runs)
    }
}
