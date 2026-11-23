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

package li.mercury.adblocker.feature.https.impl

import android.net.Uri
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.IOException
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@OptIn(ExperimentalCoroutinesApi::class)
class WorkHttpsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val done = mutableListOf<String>()
    private val written = mutableListOf<ByteArray>()
    private val https = object : HttpsRepository by unexpected() {
        override suspend fun exportWorkCertificate() = byteArrayOf(4, 5).also { done += "export work" }
    }

    @Test
    fun givenAWorkExport_whenADocumentIsChosen_thenOnlyTheWorkCertificateIsWritten() = runTest {
        val destination = Uri.parse("content://downloads/ADBlocker-work.crt")
        val work = WorkHttpsViewModel(https) { uri, bytes ->
            assertEquals(destination, uri)
            written += bytes
        }

        work.save(destination)

        assertEquals(listOf("export work"), done)
        assertContentEquals(byteArrayOf(4, 5), written.single())
        assertEquals(R.string.feature_https_work_saved, work.pendingMessages.first().single().message.text)
        assertFalse(work.busy.value)
    }

    @Test
    fun givenAnUnacknowledgedWorkExport_whenShowingTheScreenAgain_thenTheSameMessageIsAvailable() = runTest {
        val work = WorkHttpsViewModel(https) { _, _ -> }
        work.save(Uri.parse("content://downloads/ADBlocker-work.crt"))

        val first = work.pendingMessages.first().single()
        assertEquals(R.string.feature_https_work_saved, first.message.text)
        assertEquals(first, work.pendingMessages.first().single())

        work.save(Uri.parse("content://downloads/ADBlocker-work.crt"))
        val second = work.pendingMessages.value.last()
        assertTrue(first.id != second.id)
        work.onMessageShown(first.id)
        work.onMessageShown(first.id)
        assertEquals(listOf(second), work.pendingMessages.value)
        work.onMessageShown(second.id)
        assertTrue(work.pendingMessages.value.isEmpty())
    }

    @Test
    fun givenAFullQueue_whenAnExportFails_thenTheLatestFailureIsRetained() = runTest {
        var fail = false
        val work = WorkHttpsViewModel(https) { _, _ -> if (fail) throw IOException("no stream") }
        val document = Uri.parse("content://downloads/ADBlocker-work.crt")
        repeat(64) { work.save(document) }
        fail = true
        work.save(document)

        val pending = work.pendingMessages.value
        assertEquals(64, pending.size)
        assertEquals(R.string.feature_https_save_failed, pending.last().message.text)
    }

    @Test
    fun givenAnUnavailableDocument_whenWorkExportIsRetried_thenItCanSucceed() = runTest {
        var unavailable = true
        val destination = Uri.parse("content://downloads/ADBlocker-work.crt")
        val work = WorkHttpsViewModel(https) { _, bytes ->
            if (unavailable) throw IOException("no stream")
            written += bytes
        }

        work.save(destination)
        val failed = work.pendingMessages.first().single()
        assertEquals(R.string.feature_https_save_failed, failed.message.text)
        work.onMessageShown(failed.id)
        assertFalse(work.busy.value)
        assertTrue(written.isEmpty())

        unavailable = false
        work.save(destination)
        assertEquals(R.string.feature_https_work_saved, work.pendingMessages.first().single().message.text)
        assertContentEquals(byteArrayOf(4, 5), written.single())
    }

    @Test
    fun givenAnExportIsInProgress_whenSaveIsRequestedAgain_thenOnlyTheFirstDocumentIsWritten() = runTest {
        val first = Uri.parse("content://downloads/first.crt")
        val second = Uri.parse("content://downloads/second.crt")
        val finishWrite = CompletableDeferred<Unit>()
        val destinations = mutableListOf<Uri>()
        val viewModel = WorkHttpsViewModel(https) { uri, _ ->
            destinations += uri
            finishWrite.await()
        }

        viewModel.save(first)
        runCurrent()
        assertTrue(viewModel.busy.value)
        viewModel.save(second)
        runCurrent()
        assertEquals(listOf(first), destinations)
        assertEquals(listOf("export work"), done)
        assertTrue(viewModel.pendingMessages.value.isEmpty())

        finishWrite.complete(Unit)
        runCurrent()
        assertFalse(viewModel.busy.value)
        assertEquals(R.string.feature_https_work_saved, viewModel.pendingMessages.value.single().message.text)
    }

    @Test
    fun givenAnExportIsInProgress_whenTheViewModelIsCancelled_thenBusyClearsWithoutAnOutcome() = runTest {
        var writeCancelled = false
        val viewModel = WorkHttpsViewModel(https) { _, _ ->
            try {
                awaitCancellation()
            } finally {
                writeCancelled = true
            }
        }

        viewModel.save(Uri.parse("content://downloads/work.crt"))
        runCurrent()
        assertTrue(viewModel.busy.value)
        viewModel.viewModelScope.cancel()
        runCurrent()

        assertTrue(writeCancelled)
        assertFalse(viewModel.busy.value)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }
}
