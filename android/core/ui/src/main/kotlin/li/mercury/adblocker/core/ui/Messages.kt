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

package li.mercury.adblocker.core.ui

import androidx.annotation.StringRes
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalResources
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicLong

/**
 * A short message about what an action did, shown in a snackbar, with an
 * optional action such as Undo. With a [quantity], [text] is a plurals
 * resource, chosen by it.
 */
class UiMessage(
    val text: Int,
    val args: List<Any> = emptyList(),
    @param:StringRes val actionLabel: Int? = null,
    val quantity: Int? = null,
    val onAction: (() -> Unit)? = null,
)

data class PendingMessage(val id: Long, val message: UiMessage)

/** Messages kept by the screen's ViewModel until handed to the snackbar host. */
class MessageQueue {
    private val state = MutableStateFlow<List<PendingMessage>>(emptyList())
    val pending = state.asStateFlow()

    fun show(message: UiMessage) {
        val pending = PendingMessage(ids.getAndIncrement(), message)
        // A new message replaces the one shown. Keep the most recent outcomes if the screen is away.
        state.update { it.takeLast(MAX_PENDING - 1) + pending }
    }

    fun onMessageShown(id: Long) {
        state.update { messages -> messages.filterNot { it.id == id } }
    }

    private companion object {
        const val MAX_PENDING = 64
        val ids = AtomicLong()
    }
}

/**
 * The app's one snackbar state, so that a message outlives the screen that
 * asked for it (a site trusted from a sheet is confirmed after it closes).
 */
val LocalSnackbarHostState = staticCompositionLocalOf<SnackbarHostState> { error("No snackbar host") }

/** Where messages without an action are shown from: the app's scope, which outlives every screen (see [MessageEffect]). */
val LocalMessageScope = staticCompositionLocalOf<CoroutineScope?> { null }

/**
 * Whether this screen shows the snackbar: only the front pane does, so that
 * a list and its detail side by side do not both show it, and a sheet leaves
 * it to the screen it covers.
 */
val LocalHostsSnackbar = compositionLocalOf { true }

/**
 * Show pending outcomes in order, acknowledging each ID after the host takes it.
 * Non-action messages use the app's scope so they survive navigation. Actions
 * use the owning screen's scope so Undo closes with that screen. The front
 * pane's host renders the current snackbar from either scope.
 */
@Composable
fun MessageEffect(messages: StateFlow<List<PendingMessage>>, onAcknowledge: (Long) -> Unit) {
    val host = LocalSnackbarHostState.current
    val resources = LocalResources.current
    val screenScope = rememberCoroutineScope()
    val appScope = LocalMessageScope.current ?: screenScope
    LaunchedEffect(messages, host) {
        messages.collect {
            val pending = messages.value.firstOrNull() ?: return@collect
            val message = pending.message
            val args = message.args.toTypedArray()
            val text = message.quantity?.let { resources.getQuantityString(message.text, it, *args) } ?: resources.getString(message.text, *args)
            val actionLabel = message.actionLabel?.let(resources::getString)
            val scope = if (message.onAction != null) screenScope else appScope
            host.currentSnackbarData?.dismiss()
            // Register the snackbar before acknowledgement, even if acknowledgement lets the screen close.
            scope.launch(start = CoroutineStart.UNDISPATCHED) {
                val result = host.showSnackbar(message = text, actionLabel = actionLabel, withDismissAction = actionLabel != null)
                if (result == SnackbarResult.ActionPerformed) message.onAction?.invoke()
            }
            onAcknowledge(pending.id)
        }
    }
}
