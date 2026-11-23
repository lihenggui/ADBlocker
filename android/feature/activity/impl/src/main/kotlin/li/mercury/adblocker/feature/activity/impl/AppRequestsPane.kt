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

package li.mercury.adblocker.feature.activity.impl

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.animatedRow

/**
 * An app's recorded requests, as a supporting pane beside its page in extra
 * large windows (M3: a standard side sheet, at most 400dp). The rows are the
 * Activity page's; a tap on one, or on the button at the top, opens that
 * page for this app.
 */
@Composable
fun AppRequestsPane(viewModel: AppRequestsViewModel, onOpenActivity: () -> Unit, modifier: Modifier = Modifier) {
    val state by viewModel.uiState.collectAsStateWithLifecycle(minActiveState = Lifecycle.State.RESUMED)
    AppRequestsPane(
        state = state,
        onOpenActivity = {
            viewModel.showInActivity()
            onOpenActivity()
        },
        modifier = modifier,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AppRequestsPane(state: AppRequestsUiState, onOpenActivity: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        TopAppBar(
            title = { Text(stringResource(R.string.feature_activity_app_requests)) },
            actions = {
                IconButton(onClick = onOpenActivity) {
                    Icon(AdbIcons.OpenInNew, contentDescription = stringResource(R.string.feature_activity_open_activity))
                }
            },
        )
        FadeThrough(state, { it.events.isEmpty() }, Modifier.fillMaxSize(), label = "appRequests") { state ->
            if (state.events.isEmpty()) EmptyRequests(state.recording) else Requests(state, onOpenActivity)
        }
    }
}

@Composable
private fun EmptyRequests(recording: RecordingStatus) {
    Placeholder(
        AdbIcons.ReceiptLong,
        stringResource(
            when (recording) {
                RecordingStatus.Idle -> R.string.feature_activity_no_recording
                RecordingStatus.Stopped -> R.string.feature_activity_empty_result
                else -> R.string.feature_activity_empty
            },
        ),
        text = if (recording == RecordingStatus.Idle) stringResource(R.string.feature_activity_app_requests_hint) else null,
    )
}

@Composable
private fun Requests(state: AppRequestsUiState, onOpenActivity: () -> Unit) {
    LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 16.dp), modifier = Modifier.fillMaxSize()) {
        itemsIndexed(state.events, key = { _, it -> it.id }, contentType = { _, _ -> "request" }) { index, event ->
            RequestRow(
                event = event,
                app = state.app,
                position = RequestRowPosition.of(index, state.events.size),
                selected = false,
                onClick = onOpenActivity,
                modifier = Modifier.animatedRow(this, MaterialTheme.motionScheme),
            )
        }
    }
}
