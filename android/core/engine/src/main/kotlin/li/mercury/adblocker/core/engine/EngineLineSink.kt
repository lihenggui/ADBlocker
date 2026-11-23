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

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.channels.ClosedSendChannelException
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.channels.trySendBlocking
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull

/** The native writer can wait for room without holding up packet processing. */
internal class EngineLineSink(
    private val output: SendChannel<String>,
    private val writeTimeoutMillis: Long?,
) : LineSink {
    override fun line(text: String) {
        if (writeTimeoutMillis == null) {
            output.trySendBlocking(text)
            return
        }
        val offered = output.trySend(text)
        if (offered.isSuccess || offered.isClosed) return

        // The native feed can time out independently, but it cannot interrupt a
        // JNI callback. Give this send a deadline so the writer can return too.
        val sent = try {
            runBlocking { withTimeoutOrNull(writeTimeoutMillis) { output.send(text) } }
        } catch (_: ClosedSendChannelException) {
            return
        } catch (_: CancellationException) {
            return
        }
        if (sent == null) output.close(EngineExit(1, ""))
    }

    override fun end(exit: Int) {
        if (exit == 0) output.close() else output.close(EngineExit(exit, ""))
    }
}
