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
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.flow.updateAndGet
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DocumentWriter
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import javax.inject.Inject

enum class HttpsStep { Impact, Install, Confirm, Result }

data class HttpsUiState(
    val step: HttpsStep = HttpsStep.Impact,
    /** The certificate was saved for the system's installer. */
    val saved: Boolean = false,
    /** Back from the system settings, and the certificate is not in the user store. */
    val notFound: Boolean = false,
    val busy: Boolean = false,
    /** Every filtered app as the engine has it once decryption is on. */
    val results: ReadState<List<AppEntry>> = ReadState.Pending,
)

/**
 * The HTTPS setup without root. Android does not let an app install a CA
 * certificate: it is saved for the user to install in the system settings,
 * and looked for in the user store when the app is back.
 */
@HiltViewModel
class HttpsViewModel @Inject constructor(
    private val https: HttpsRepository,
    private val apps: AppsRepository,
    private val daemon: DaemonRepository,
    private val writer: DocumentWriter,
    private val savedState: SavedStateHandle,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
) : ViewModel() {
    // The system settings, where the certificate is installed, can cost the
    // process: the step, whether the certificate was saved and whether the
    // app waits for the install come back with it. They name no host or app.
    private val state = MutableStateFlow(restoredState())
    private var initialCheckComplete = false
    val uiState: StateFlow<HttpsUiState> = state.onStart {
        if (!initialCheckComplete) {
            // Returning to the screen should keep the step the user reached.
            if (installed() && state.value.step == HttpsStep.Impact) go(HttpsStep.Confirm)
            initialCheckComplete = true
        }
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), state.value)
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)

    /** The system settings were opened to install the certificate, and it has not been found since. */
    private var awaitingInstall: Boolean
        get() = savedState[AWAITING_INSTALL] ?: false
        set(value) {
            savedState[AWAITING_INSTALL] = value
        }

    private fun restoredState() = HttpsUiState(
        step = savedState.get<String>(STEP)?.let { name -> HttpsStep.entries.firstOrNull { it.name == name } } ?: HttpsStep.Impact,
        saved = savedState[SAVED] ?: false,
    )

    /** Changes the state, and what comes back with the process. */
    private fun change(update: (HttpsUiState) -> HttpsUiState) {
        val next = state.updateAndGet(update)
        // The results are not kept: coming back to the last step is asking again.
        savedState[STEP] = (if (next.step == HttpsStep.Result) HttpsStep.Confirm else next.step).name
        savedState[SAVED] = next.saved
    }

    /** Reads the key store, off the main thread. */
    private suspend fun installed() = withContext(io) { https.certificateInstalled() }

    /** From what it costs to installing; the rest moves on as it is done. */
    fun next() {
        if (state.value.step == HttpsStep.Impact) go(HttpsStep.Install)
    }

    fun previous() {
        when (state.value.step) {
            HttpsStep.Install -> go(HttpsStep.Impact)
            HttpsStep.Confirm -> go(HttpsStep.Install)
            HttpsStep.Impact, HttpsStep.Result -> Unit
        }
    }

    private fun go(step: HttpsStep) {
        change { it.copy(step = step, notFound = false) }
    }

    /** Saves the certificate where the user chose, for the system's installer. */
    fun save(uri: Uri) = busy(R.string.feature_https_save_failed) {
        writer.write(uri, https.exportCertificate())
        change { it.copy(saved = true) }
    }

    /** The system settings are being opened: the certificate is looked for when the app is back. */
    fun openingSettings() {
        awaitingInstall = true
        state.update { it.copy(notFound = false) }
    }

    /** Back in the app: whether the certificate was installed meanwhile. */
    fun checkInstalled() {
        if (state.value.step != HttpsStep.Install || !awaitingInstall) return
        viewModelScope.launch {
            if (installed()) {
                awaitingInstall = false
                reloadForCertificate()
                go(HttpsStep.Confirm)
            } else {
                state.update { it.copy(notFound = true) }
            }
        }
    }

    /**
     * The engine looks for the certificate when it reloads. URL filtering is
     * usually on already, so leaving the setup here would change no setting
     * that makes it look again.
     */
    private suspend fun reloadForCertificate() {
        try {
            https.reload()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Confirming the setup reloads again and reports a failure there.
        }
    }

    fun turnOn() = busy(R.string.feature_https_failed) {
        if (!installed()) {
            awaitingInstall = true
            change { it.copy(step = HttpsStep.Install, notFound = true) }
            return@busy
        }
        daemon.setUrlFiltering(true)
        https.reload()
        val results = readSection { apps.apps().filter { it.app.profileSerial == null && it.isFiltered } }
        change { it.copy(step = HttpsStep.Result, results = results) }
    }

    private fun busy(failure: Int, block: suspend () -> Unit) {
        if (state.value.busy) return
        viewModelScope.launch {
            state.update { it.copy(busy = true) }
            try {
                suspendRunCatching("https") {
                    block()
                }.onFailure {
                    messages.show(UiMessage(failure))
                }
            } finally {
                state.update { it.copy(busy = false) }
            }
        }
    }

    private companion object {
        const val STEP = "https_step"
        const val SAVED = "https_saved"
        const val AWAITING_INSTALL = "https_awaiting_install"
    }
}
