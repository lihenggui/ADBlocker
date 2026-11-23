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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

class EngineLineSinkTest {
    @Test
    fun givenStalledCollector_whenWatchBufferFills_thenTheWatchEnds() = runTest(timeout = 5.seconds) {
        val opened = CompletableDeferred<LineSink>()
        val cancelled = CompletableDeferred<Unit>()
        val reading = CompletableDeferred<Unit>()
        val resume = CompletableDeferred<Unit>()
        val watch = callbackFlow {
            opened.complete(EngineLineSink(channel, writeTimeoutMillis = 25))
            awaitClose { cancelled.complete(Unit) }
        }.buffer(1)
        val collected = async {
            assertFailsWith<EngineExit> {
                watch.collect { line ->
                    if (line == "first") {
                        reading.complete(Unit)
                        resume.await()
                    }
                }
            }
        }

        try {
            val writer = opened.await()
            withContext(Dispatchers.IO) { writer.line("first") }
            reading.await()
            val writing = async(Dispatchers.IO) {
                writer.line("buffered")
                writer.line("stalled")
            }
            cancelled.await()
            writing.await()
            assertTrue(collected.isActive)
        } finally {
            resume.complete(Unit)
        }
        assertEquals(1, collected.await().exit)
    }
}
