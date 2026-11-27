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
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.data.repository.allowRule
import li.mercury.adblocker.core.data.repository.blockRule
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.Verdict
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
    /** The current rule match for the recorded URL as a GET, or the blocked domain. */
    val rule: RuleCheckResult? = null,
    val ruleCheckPending: Boolean = false,
    val ruleCheckUnavailable: Boolean = false,
    val done: Boolean = false,
    val userAllowRule: String? = null,
) {
    internal val ruleToRemove: String? = when {
        request == null || request.verdict == Verdict.Passed || rule?.hasExceptionField != true -> null
        rule.blocked && rule.source == RuleSource.UserRules -> rule.rule
        !rule.blocked && rule.exceptionSource == RuleSource.UserRules -> rule.exception
        else -> userAllowRule
    }

    internal val canBlock: Boolean = request?.verdict == Verdict.Allowed && ruleToRemove == null && !ruleCheckPending

    private val blockedHost: String? = request
        ?.takeIf { it.blocked && !ruleCheckPending }
        ?.let { exceptionHost(it.host) }

    private val removesGlobalBlock = ruleToRemove != null && rule?.blocked == true && rule.source == RuleSource.UserRules &&
        ruleToRemove.substringAfter('$', "").splitToSequence('$', ',').none { it.startsWith("app=") }

    internal val allowHost: String? = blockedHost.takeIf { ruleToRemove == null }
    internal val allowHostInApp: String? = blockedHost.takeIf { rulePackage != null && (ruleToRemove == null || removesGlobalBlock) }

    internal val displayedRule: String? = ruleToRemove ?: rule?.rule?.takeIf { request?.url != null }
    internal val displayedRuleSource: RuleSource? = if (ruleToRemove != null) RuleSource.UserRules else rule?.source
}

@HiltViewModel(assistedFactory = RequestViewModel.Factory::class)
class RequestViewModel @AssistedInject constructor(
    @Assisted requestId: Long,
    recorder: ActivityRecorder,
    private val apps: AppsRepository,
    private val rules: RulesRepository,
    activityMessages: ActivityMessages = ActivityMessages(),
) : ViewModel() {
    private val messages = activityMessages.queue
    internal val pendingMessages = messages.pending

    private val request: RequestEvent? = recorder.state.value.feed.events.firstOrNull { it.id == requestId }
    private val hostToCheck = request?.takeIf { it.verdict == Verdict.HostBlocked }?.let { exceptionHost(it.host) }
    private val initial = RequestUiState(request = request, gone = request == null, ruleCheckPending = request?.url != null || hostToCheck != null)
    private val revision = MutableStateFlow(0)
    private val done = MutableStateFlow(false)
    private var action: Job? = null
    val uiState: StateFlow<RequestUiState> = flow {
        emit(initial)
        val request = request ?: return@flow
        val entry = suspendRunCatching { apps.app(request.appKey) }.getOrNull()
        val withApp = initial.copy(app = entry?.app)
        emit(withApp)
        if (initial.ruleCheckPending) emitAll(revision.map { checkRule(request, withApp) })
    }.combine(done) { state, complete -> state.copy(done = complete) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), initial)

    private suspend fun checkRule(request: RequestEvent, state: RequestUiState): RequestUiState {
        val url = request.url
        val rule = suspendRunCatching {
            if (url != null) {
                rules.checkUrl(url, request.page, request.kind, request.appKey)
            } else {
                rules.checkHost(checkNotNull(hostToCheck), request.appKey)
            }
        }
        val checked = state.copy(rule = rule.getOrNull(), ruleCheckPending = false, ruleCheckUnavailable = url != null && rule.isFailure)
        if (request.verdict != Verdict.Allowed || checked.rule?.hasExceptionField != true || checked.ruleToRemove != null) return checked

        // A host exception is absent from diagnostics when there was no block to override.
        val allowance = suspendRunCatching("read request allow rule") { findUserAllowRule(request.host, checked.rulePackage) }
        return checked.copy(userAllowRule = allowance.getOrNull(), ruleCheckUnavailable = allowance.isFailure)
    }

    private suspend fun findUserAllowRule(host: String, app: String?): String? {
        val name = exceptionHost(host) ?: return null
        val site = siteOf(name) ?: name
        val hosts = generateSequence(name) { it.takeUnless { it == site }?.substringAfter('.') }
        val candidates = buildList {
            if (app != null) hosts.forEach { add(allowRule(it, app)) }
            hosts.forEach { add(allowRule(it)) }
        }
        val candidateSet = candidates.toHashSet()
        val matches = rules.userRules().lineSequence().map(String::trim).filter(candidateSet::contains).toHashSet()
        return candidates.firstOrNull(matches::contains)
    }

    /** Trusts the site: its connections are no longer decrypted or blocked. */
    fun trustSite() {
        val site = initial.trustSite ?: return
        act {
            rules.trust(site)
            messages.show(UiMessage(R.string.feature_activity_trusted, listOf(isolateLtr(site))))
        }
    }

    fun blockHost() = blockHost(inApp = false)

    fun blockHostInApp() = blockHost(inApp = true)

    private fun blockHost(inApp: Boolean) {
        val state = uiState.value
        if (!state.canBlock) return
        val host = state.request?.host ?: return
        val app = if (inApp) state.rulePackage ?: return else null
        val text = if (inApp) R.string.feature_activity_blocked_host_in_app else R.string.feature_activity_blocked_host_everywhere
        addHostRule(blockRule(host, app), text, app)
    }

    fun allowHost() = allowHost(inApp = false)

    fun allowHostInApp() = allowHost(inApp = true)

    private fun allowHost(inApp: Boolean) {
        val state = uiState.value
        val host = (if (inApp) state.allowHostInApp else state.allowHost) ?: return
        val app = if (inApp) state.rulePackage ?: return else null
        val text = if (inApp) R.string.feature_activity_allowed_host_in_app else R.string.feature_activity_allowed_host_everywhere
        addHostRule(allowRule(host, app), text, app)
    }

    private fun addHostRule(rule: String, text: Int, app: String?) = act { request ->
        val args = listOfNotNull(isolateLtr(request.host), app?.let(::ruleAppLabel))
        val changed = rules.addRule(rule)
        showRuleEdit(text, args, changed) { rules.removeRule(rule) }
    }

    fun removeRule() {
        val rule = uiState.value.ruleToRemove ?: return
        act(closeOnSuccess = false) {
            if (!rules.removeRule(rule)) {
                revision.update { it + 1 }
                messages.show(UiMessage(R.string.feature_activity_rule_not_in_my_rules))
                return@act
            }
            showRuleEdit(R.string.feature_activity_rule_removed, emptyList(), changed = true) { rules.addRule(rule) }
            done.value = true
        }
    }

    private fun showRuleEdit(text: Int, args: List<Any>, changed: Boolean, undo: suspend () -> Unit) {
        revision.update { it + 1 }
        if (!changed) {
            messages.show(UiMessage(text, args))
            return
        }
        messages.show(
            UiMessage(text, args, actionLabel = R.string.feature_activity_undo) {
                suspendRunCatching("undo request rule", undo).onFailure { showFailure() }
                revision.update { it + 1 }
            },
        )
    }

    private fun ruleAppLabel(app: String) = appLabelArgument(uiState.value.app?.label ?: app, app)

    fun forgetPinned() = act { request ->
        apps.forgetPinned(request.appKey, request.host)
        messages.show(UiMessage(R.string.feature_activity_forgotten))
    }

    private fun act(closeOnSuccess: Boolean = true, block: suspend (RequestEvent) -> Unit) {
        val request = request ?: return
        if (action?.isActive == true) return
        action = viewModelScope.launchCatching("request action", onFailure = { showFailure() }) {
            block(request)
            if (closeOnSuccess) done.value = true
        }
    }

    private fun showFailure() = messages.show(UiMessage(R.string.feature_activity_failed))

    @AssistedFactory
    interface Factory {
        fun create(requestId: Long): RequestViewModel
    }
}
