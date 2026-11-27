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

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.assisted.Assisted
import dagger.assisted.AssistedFactory
import dagger.assisted.AssistedInject
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.stateIn
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import li.mercury.adblocker.core.ui.appLabelArgument
import li.mercury.adblocker.core.ui.isolateLtr

data class RequestUiState(
    /** The request, as the feed received it; null when it is no longer kept. */
    val request: RequestEvent? = null,
    /** The request is no longer kept, as after the process ended: the page closes. */
    val gone: Boolean = false,
    val trustSite: String? = request?.let { siteOf(it.host) },
    /** The package a rule for this app alone names. */
    val rulePackage: String? = request?.identity?.rulePackage,
    val app: InstalledApp? = null,
    /** The current rules’ result for testing the recorded URL as a GET request. */
    val rule: RuleCheckResult? = null,
    val ruleCheckUnavailable: Boolean = false,
    val done: Boolean = false,
) {
    internal val allowHost: String? = when (request?.verdict) {
        Verdict.Blocked, Verdict.HostBlocked -> exceptionHost(request.host)
        else -> null
    }
}

@HiltViewModel(assistedFactory = RequestViewModel.Factory::class)
class RequestViewModel @AssistedInject constructor(
    @Assisted requestId: Long,
    recorder: ActivityRecorder,
    private val apps: AppsRepository,
    private val rules: RulesRepository,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val request: RequestEvent? = recorder.state.value.feed.events.firstOrNull { it.id == requestId }
    private val initial = RequestUiState(request = request, gone = request == null)
    private val done = MutableStateFlow(false)
    val uiState: StateFlow<RequestUiState> = flow {
        emit(initial)
        val request = request ?: return@flow
        val entry = suspendRunCatching { apps.app(request.appKey) }.getOrNull()
        val withApp = initial.copy(app = entry?.app)
        emit(withApp)
        // Only a recorded URL can be tested against today’s rules.
        request.url?.let { url ->
            val rule = suspendRunCatching { rules.checkUrl(url, request.page, request.kind, request.appKey) }
            emit(withApp.copy(rule = rule.getOrNull(), ruleCheckUnavailable = rule.isFailure))
        }
    }.combine(done) { state, complete -> state.copy(done = complete) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), initial)

    /** Trusts the site: its connections are no longer decrypted or blocked. */
    fun trustSite() {
        val site = initial.trustSite ?: return
        act {
            rules.trust(site)
            messages.show(UiMessage(R.string.feature_activity_trusted, listOf(isolateLtr(site))))
        }
    }

    /** Blocks the request's host and every name below it, in every app. */
    fun blockHost() = act { request ->
        rules.blockDomain(request.host)
        messages.show(UiMessage(R.string.feature_activity_blocked_host_everywhere, listOf(isolateLtr(request.host))))
    }

    /** Blocks the request's host and every name below it in the app that made the request, in each of its profiles. */
    fun blockHostInApp() {
        val app = initial.rulePackage ?: return
        act { request ->
            rules.blockDomain(request.host, app)
            messages.show(UiMessage(R.string.feature_activity_blocked_host_in_app, listOf(isolateLtr(request.host), ruleAppLabel(app))))
        }
    }

    fun allowHost() {
        val host = initial.allowHost ?: return
        act { request ->
            rules.allowDomain(host)
            messages.show(UiMessage(R.string.feature_activity_allowed_host_everywhere, listOf(isolateLtr(request.host))))
        }
    }

    fun allowHostInApp() {
        val host = initial.allowHost ?: return
        val app = initial.rulePackage ?: return
        act { request ->
            rules.allowDomain(host, app)
            messages.show(UiMessage(R.string.feature_activity_allowed_host_in_app, listOf(isolateLtr(request.host), ruleAppLabel(app))))
        }
    }

    private fun ruleAppLabel(app: String) = appLabelArgument(uiState.value.app?.label ?: app, app)

    fun forgetPinned() = act { request ->
        apps.forgetPinned(request.appKey, request.host)
        messages.show(UiMessage(R.string.feature_activity_forgotten))
    }

    private fun act(block: suspend (RequestEvent) -> Unit) {
        val request = request ?: return
        viewModelScope.launchCatching("request action", onFailure = {
            messages.show(UiMessage(R.string.feature_activity_failed))
        }) {
            block(request)
            done.value = true
        }
    }

    @AssistedFactory
    interface Factory {
        fun create(requestId: Long): RequestViewModel
    }
}
