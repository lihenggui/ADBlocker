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
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption.ATOMIC_MOVE
import java.nio.file.StandardCopyOption.REPLACE_EXISTING
import java.util.UUID
import javax.inject.Inject
import kotlin.time.Duration.Companion.days

/** Checkpoints unsaved rule text in private, non-backed-up storage. */
interface RuleDraftStore {
    suspend fun read(id: UUID): String?

    /** Replaces the checkpoint, or removes it when [text] is null. */
    suspend fun write(id: UUID, text: String?)
}

internal class FileRuleDraftStore(
    private val directory: File,
    private val io: CoroutineDispatcher,
) : RuleDraftStore {
    @Inject
    constructor(@ApplicationContext context: Context, @Dispatcher(IO) io: CoroutineDispatcher) :
        this(File(context.noBackupFilesDir, "rule-editor-drafts"), io)

    override suspend fun read(id: UUID): String? = withContext(io) {
        val draft = File(directory, "$id.txt")
        val temporary = File(directory, "$id.tmp")
        // A process can end before removing a draft for an entry that never returns.
        val staleBefore = System.currentTimeMillis() - 7.days.inWholeMilliseconds
        directory.listFiles()?.forEach { if (it != draft && it.lastModified() < staleBefore) it.delete() }
        temporary.delete()
        if (!draft.exists()) return@withContext null
        if (draft.length() > MAX_RULES_BYTES) throw RulesTooLargeException()
        draft.readText()
    }

    override suspend fun write(id: UUID, text: String?): Unit = withContext(io) {
        val draft = File(directory, "$id.txt")
        if (text == null) {
            Files.deleteIfExists(draft.toPath())
            return@withContext
        }
        if (text.length > MAX_RULES_BYTES) throw RulesTooLargeException()
        val bytes = text.encodeToByteArray()
        if (bytes.size > MAX_RULES_BYTES) throw RulesTooLargeException()
        val temporary = File(directory, "$id.tmp")
        Files.createDirectories(directory.toPath())
        try {
            temporary.writeBytes(bytes)
            // Recovery sees the whole latest draft or the previous one.
            Files.move(temporary.toPath(), draft.toPath(), ATOMIC_MOVE, REPLACE_EXISTING)
        } finally {
            temporary.delete()
        }
    }
}
