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

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.retryWhen
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.DEFAULT
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.WatchEvent
import javax.inject.Inject

/** The most events kept in a recording. */
const val MAX_ACTIVITY_EVENTS = 500

/**
 * How often in a row an ended watch is started again, waiting 1, 2, 4, 8 and
 * 16 s. A watch that delivered an event starts the count again.
 */
internal const val WATCH_RETRIES = 5
private const val WATCH_BACKOFF_MS = 1_000L
internal const val ACTIVITY_SNAPSHOT_MS = 150L

/** Events newest first, and how many were dropped by the daemon or evicted from this feed. */
data class ActivityFeed(val events: List<RequestEvent>, val dropped: Long) {
    companion object {
        val Empty = ActivityFeed(emptyList(), 0)
    }
}

/** Insertion and eviction are constant time. A reader takes an independent snapshot. */
class ActivityBuffer {
    private val events = ArrayDeque<RequestEvent>(MAX_ACTIVITY_EVENTS)
    private var dropped = 0L

    @Synchronized
    internal fun add(event: WatchEvent) {
        when (event) {
            is WatchEvent.Request -> {
                if (events.size == MAX_ACTIVITY_EVENTS) {
                    events.removeLast()
                    dropped++
                }
                events.addFirst(event.event)
            }

            is WatchEvent.Dropped -> dropped += event.count
        }
    }

    @Synchronized
    fun snapshot(): ActivityFeed = ActivityFeed(events.toList(), dropped)
}

/**
 * The filtered apps' requests, live. They are kept in memory only, at most
 * [MAX_ACTIVITY_EVENTS] of them, and only while the feed is collected: what
 * the apps talk to is never written by the app.
 */
interface ActivityRepository {
    fun feed(packageName: String?, buffer: ActivityBuffer? = null): Flow<ActivityFeed>
}

internal class AbdActivityRepository @Inject constructor(
    private val abd: AbdClient,
    @Dispatcher(DEFAULT) private val defaultDispatcher: CoroutineDispatcher,
) : ActivityRepository {
    // The daemon ends a watch when it restarts, or when the app read nothing
    // for a while: the watch starts again, a few times in a row, waiting
    // longer each time; the events seen so far stay. A busy app sends many
    // events a second: they are read and kept off the main thread.
    override fun feed(packageName: String?, buffer: ActivityBuffer?): Flow<ActivityFeed> = channelFlow {
        val recorded = buffer ?: ActivityBuffer()
        val changed = Channel<Unit>(Channel.CONFLATED)
        send(ActivityFeed.Empty)
        launch {
            var failures = 0
            try {
                abd.watch(packageName)
                    .onEach { failures = 0 }
                    .retryWhen { cause, _ ->
                        val again = cause is AbdException.CommandFailed && failures < WATCH_RETRIES
                        if (again) delay(WATCH_BACKOFF_MS shl failures++)
                        again
                    }
                    .collect { event ->
                        recorded.add(event)
                        changed.trySend(Unit)
                    }
            } finally {
                changed.close()
            }
        }
        while (changed.receiveCatching().isSuccess) {
            send(recorded.snapshot())
            delay(ACTIVITY_SNAPSHOT_MS)
        }
    }.conflate().flowOn(defaultDispatcher)
}
