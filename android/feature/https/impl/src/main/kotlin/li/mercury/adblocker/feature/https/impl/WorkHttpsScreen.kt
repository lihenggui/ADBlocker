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
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.DocumentWriter
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.MaxContentWidth
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import javax.inject.Inject

@HiltViewModel
internal class WorkHttpsViewModel @Inject constructor(
    private val https: HttpsRepository,
    private val writer: DocumentWriter,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)

    private val saving = MutableStateFlow(false)
    val busy = saving.asStateFlow()

    fun save(uri: Uri) {
        if (!saving.compareAndSet(expect = false, update = true)) return
        viewModelScope.launch {
            try {
                val saved = suspendRunCatching { writer.write(uri, https.exportWorkCertificate()) }.isSuccess
                messages.show(UiMessage(if (saved) R.string.feature_https_work_saved else R.string.feature_https_save_failed))
            } finally {
                saving.value = false
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun WorkHttpsScreen(onBack: () -> Unit, viewModel: WorkHttpsViewModel) {
    val busy by viewModel.busy.collectAsStateWithLifecycle()
    MessageEffect(viewModel.pendingMessages, viewModel::onMessageShown)
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/x-x509-ca-cert")) { uri ->
        uri?.let(viewModel::save)
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.feature_https_work_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(AdbIcons.ArrowBack, contentDescription = stringResource(R.string.feature_https_back))
                    }
                },
            )
        },
        snackbarHost = { AppSnackbarHost() },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = MaxContentWidth).verticalScroll(rememberScrollState()).padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Text(stringResource(R.string.feature_https_work_instructions), style = MaterialTheme.typography.bodyLarge)
                Button(onClick = { save.launch("ADBlocker-work.crt") }, enabled = !busy) {
                    Text(stringResource(R.string.feature_https_work_save))
                }
            }
        }
    }
}
