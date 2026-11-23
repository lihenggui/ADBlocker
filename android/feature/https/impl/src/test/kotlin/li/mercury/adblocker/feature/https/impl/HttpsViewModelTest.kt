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
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
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

// Under Robolectric for Uri, which the certificate is saved to.
@RunWith(RobolectricTestRunner::class)
@OptIn(ExperimentalCoroutinesApi::class)
class HttpsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private fun app(name: String) = InstalledApp("com.example.$name", name, AppIconSource("com.example.$name", "test"), isSystem = false)

    /** What was done, in order: the order is what the tests are about. */
    private val done = mutableListOf<String>()

    private var installed = false
    private var certificateReads = 0

    /** On, as the engine has it until the user turns it off. */
    private var urlFiltering = true

    private var appReadFailure: AbdException? = null
    private var selected = setOf("com.example.browser", "com.example.news", "com.example.bank")
    private val installedApps = listOf(app("browser"), app("news"), app("bank"), app("weather"))

    private val https = object : HttpsRepository {
        override suspend fun exportCertificate() = byteArrayOf(1, 2, 3).also { done += "export" }

        override suspend fun exportWorkCertificate(): ByteArray = error("personal setup cannot export for work")

        override fun certificateInstalled() = installed.also { certificateReads++ }

        override suspend fun reload() {
            done += "reload"
        }
    }

    private val apps = object : AppsRepository by unexpected() {
        override suspend fun apps(): List<AppEntry> {
            appReadFailure?.let { throw it }
            return installedApps.map { app ->
                AppEntry(
                    app,
                    selected.find { it == app.packageName }?.let {
                        val level = if (urlFiltering && installed) FilterLevel.Full else FilterLevel.Hosts
                        FilteredApp(app.packageName, level, pausedUntil = null)
                    },
                )
            }
        }
    }

    private val daemon = object : DaemonRepository by unexpected() {
        override suspend fun status() = DaemonStatus(
            running = true, pid = null, startedAt = null, version = null, apps = 3, rules = 1, hostRules = null,
            exclusions = null, paused = false, resumeAt = null, ipv6 = null, listsUpdated = null, urlFiltering = urlFiltering,
        )

        override suspend fun setUrlFiltering(on: Boolean) {
            done += "urlFiltering $on"
            urlFiltering = on
        }
    }

    private val written = mutableListOf<ByteArray>()

    private val viewModel by lazy { HttpsViewModel(https, apps, daemon, { _, bytes -> written += bytes }, main.testDispatcher) }

    private fun TestScope.show() = backgroundScope.launch { viewModel.uiState.collect {} }.also { runCurrent() }

    @Test
    fun givenAppResultsCannotBeRead_whenUrlFilteringIsEnabled_thenTheSetupStillFinishesWithUnavailableResults() = runTest {
        installed = true
        appReadFailure = AbdException.TimedOut()
        show()

        viewModel.turnOn()
        runCurrent()

        val state = viewModel.uiState.value
        assertEquals(listOf("urlFiltering true", "reload"), done)
        assertEquals(HttpsStep.Result, state.step)
        assertFalse(state.busy)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), state.results)
    }

    @Test
    fun givenThereAreNoSelectedApps_whenUrlFilteringIsEnabled_thenTheResultsAreAvailableAndEmpty() = runTest {
        installed = true
        selected = emptySet()
        show()

        viewModel.turnOn()
        runCurrent()

        assertEquals(HttpsStep.Result, viewModel.uiState.value.step)
        assertEquals(ReadState.Available(emptyList<AppEntry>()), viewModel.uiState.value.results)
    }

    @Test
    fun givenCertificateExportFails_whenTheOutcomeIsCollectedAgain_thenItWaitsForAcknowledgement() = runTest {
        val viewModel = HttpsViewModel(https, apps, daemon, { _, _ -> throw IOException("no stream") }, main.testDispatcher)
        viewModel.save(Uri.parse("content://downloads/ADBlocker.crt"))
        runCurrent()

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_https_save_failed, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)
    }

    @Test
    fun givenAnInstalledCertificate_whenTheScreenIsCollectedAgain_thenSetupKeepsItsProgress() = runTest {
        installed = true
        assertEquals(HttpsStep.Impact, viewModel.uiState.value.step)
        assertEquals(0, certificateReads)
        val screen = show()
        assertEquals(HttpsStep.Confirm, viewModel.uiState.value.step)
        viewModel.previous()
        screen.cancel()
        advanceTimeBy(5_001)
        show()
        assertEquals(HttpsStep.Install, viewModel.uiState.value.step)
        assertEquals(1, certificateReads)
    }

    @Test
    fun theSetupStartsWithWhatItCostsAndGoesToTheAppsOnceTheCertificateIsFoundOnReturn() = runTest {
        show()
        assertEquals(HttpsStep.Impact, viewModel.uiState.value.step)
        viewModel.next()
        viewModel.save(Uri.parse("content://downloads/ADBlocker.crt"))
        assertTrue(viewModel.uiState.value.saved)
        assertEquals(listOf("export"), done)
        assertContentEquals(byteArrayOf(1, 2, 3), written.single())

        // Back from the settings without it: said so, and still on this step.
        viewModel.openingSettings()
        viewModel.checkInstalled()
        assertEquals(HttpsStep.Install, viewModel.uiState.value.step)
        assertTrue(viewModel.uiState.value.notFound)

        installed = true
        viewModel.openingSettings()
        viewModel.checkInstalled()
        assertEquals(HttpsStep.Confirm, viewModel.uiState.value.step)
        // The engine is told to look for the certificate without waiting for the last step.
        assertEquals(listOf("export", "reload"), done)
    }

    @Test
    fun aCertificateInstalledAlreadyLeadsStraightToTheApps() = runTest {
        installed = true
        show()
        assertEquals(HttpsStep.Confirm, viewModel.uiState.value.step)
    }

    @Test
    fun givenAnInstalledCertificate_whenUrlFilteringIsEnabled_thenItAppliesToAllSelectedApps() = runTest {
        installed = true
        show()
        viewModel.turnOn()

        assertEquals(listOf("urlFiltering true", "reload"), done)
        val state = viewModel.uiState.value
        assertEquals(HttpsStep.Result, state.step)
        val results = requireNotNull(state.results.valueOrNull)
        assertEquals(selected, results.map { it.packageName }.toSet())
        assertTrue(results.all { it.filter?.level == FilterLevel.Full })
    }

    @Test
    fun givenUrlFilteringTurnedOff_whenTheSetupIsConfirmed_thenItIsOnAgainForAllSelectedApps() = runTest {
        installed = true
        urlFiltering = false
        show()
        viewModel.turnOn()

        assertTrue(urlFiltering)
        assertEquals(listOf("urlFiltering true", "reload"), done)
        assertTrue(requireNotNull(viewModel.uiState.value.results.valueOrNull).all { it.filter?.level == FilterLevel.Full })
    }
}
