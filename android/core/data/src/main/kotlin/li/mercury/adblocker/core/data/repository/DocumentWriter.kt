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

import android.content.Context
import android.net.Uri
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import java.io.IOException
import javax.inject.Inject

/** Writes [bytes] to a document the user chose. */
fun interface DocumentWriter {
    suspend fun write(uri: Uri, bytes: ByteArray)
}

/** Through the content resolver, off the main thread. */
internal class ContentDocumentWriter @Inject constructor(
    @ApplicationContext private val context: Context,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
) : DocumentWriter {
    override suspend fun write(uri: Uri, bytes: ByteArray) = withContext(io) {
        val stream = context.contentResolver.openOutputStream(uri, "wt") ?: throw IOException("no stream")
        stream.use { it.write(bytes) }
    }
}
