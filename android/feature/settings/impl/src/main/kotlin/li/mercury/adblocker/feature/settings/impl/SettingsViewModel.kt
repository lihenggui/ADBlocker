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

package li.mercury.adblocker.feature.settings.impl

import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.flow.refreshWhileCollected
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.apps.AppPlatformInfo
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.DiagnosticsRepository
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.LogExportResult
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import javax.inject.Inject
import kotlin.reflect.KProperty1

/** What the settings page shows of the daemon and the module. */
data class ServiceInfo(
    val status: ReadState<DaemonStatus> = ReadState.Pending,
    val module: ModuleState? = null,
    val moduleVersion: String? = null,
    val rootManager: String? = null,
    val pinned: ReadState<List<PinnedHost>> = ReadState.Pending,
    val busy: Boolean = false,
    /** Without root: whether the certificate is in the user store. */
    val certificateInstalled: Boolean = false,
    /** Without root: whether root is granted, so that the root module can be chosen; null until asked. */
    val rootAvailable: Boolean? = null,
    val checkingRoot: Boolean = false,
    /** Whether the app may show notifications: the VPN's, with its pause action. */
    val notificationsAllowed: Boolean = true,
    val pinnedLabels: Map<String, String> = emptyMap(),
)

data class SettingsUiState(
    val userData: UserData = UserData(),
    val service: ServiceInfo = ServiceInfo(),
    val appVersion: String = "",
    /** Without root: the VPN's state and the system's settings for it. */
    val vpn: VpnStatus = VpnStatus(),
)

@HiltViewModel
class SettingsViewModel @Inject constructor(
    private val platform: AppPlatformInfo,
    private val userData: UserDataRepository,
    private val daemon: DaemonRepository,
    private val module: ModuleRepository,
    private val apps: AppsRepository,
    private val diagnostics: DiagnosticsRepository,
    private val methods: FilterMethodRepository,
    private val https: HttpsRepository,
    private val vpn: VpnController,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
    changes: DataChanges,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val service = MutableStateFlow(ServiceInfo())

    // Read with the first load: asking the package manager does not belong in construction.
    private val appVersion = MutableStateFlow("")
    private val changingHttps = mutableMapOf<KProperty1<DaemonStatus, Boolean?>, Boolean>()
    private var httpsRevision = 0L

    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
    val uiState: StateFlow<SettingsUiState> = combine(userData.userData, service, vpn.status, appVersion) { data, info, vpnStatus, version -> SettingsUiState(data, info, version, vpnStatus) }
        .refreshWhileCollected(merge(changes.changes, reload), ::load)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), SettingsUiState())

    fun refresh() {
        reload.tryEmit(Unit)
    }

    private suspend fun load() {
        val revision = httpsRevision
        if (appVersion.value.isEmpty()) appVersion.value = suspendRunCatching { platform.versionName() }.getOrDefault("")
        if (userData.userData.first().method == FilterMethod.Vpn) {
            readVpn(revision)
            return
        }
        val moduleState = readModule()
        val status = readSection { daemon.status() }
        val moduleVersion = suspendRunCatching { module.installedVersion() }.getOrNull()
        val rootManager = suspendRunCatching { module.rootManager() }.getOrNull()
        val pinned = readSection { apps.pinned() }
        val labels = pinned.valueOrNull?.let { pinnedLabels(it) }.orEmpty()
        service.update {
            it.copy(
                module = moduleState,
                status = statusAfterRead(it.status, status, revision),
                moduleVersion = moduleVersion,
                rootManager = rootManager,
                pinned = pinned,
                pinnedLabels = labels,
            )
        }
    }

    /**
     * Without root: the engine's state, the certificate and the
     * notifications. Nothing here runs a root command, which would show the
     * root manager's prompt; root is asked for once, and when asked again.
     */
    private suspend fun readVpn(revision: Long) {
        val status = readSection { daemon.status() }
        val pinned = readSection { apps.pinned() }
        val labels = pinned.valueOrNull?.let { pinnedLabels(it) }.orEmpty()
        val installed = withContext(io) { https.certificateInstalled() }
        val notifications = platform.notificationsAllowed()
        val moduleState = if (userData.userData.first().rootSwitchPending) readModule() else null
        service.update {
            it.copy(
                status = statusAfterRead(it.status, status, revision),
                module = moduleState,
                pinned = pinned,
                certificateInstalled = installed,
                notificationsAllowed = notifications,
                pinnedLabels = labels,
            )
        }
        if (service.value.rootAvailable == null) readRoot(reportMissing = false)
    }

    private fun statusAfterRead(before: ReadState<DaemonStatus>, read: ReadState<DaemonStatus>, revision: Long): ReadState<DaemonStatus> = if (changingHttps.isNotEmpty() || httpsRevision != revision) before else read

    private suspend fun readModule(): ModuleState = suspendRunCatching { module.state() }.getOrElse {
        if (it is AbdException.NoRoot) ModuleState.NoRoot else ModuleState.Unavailable
    }

    private suspend fun pinnedLabels(pinned: List<PinnedHost>): Map<String, String> {
        val packages = pinned.asSequence()
            .filter { it.profileSerial == null && !it.packageName.startsWith('@') }
            .map(PinnedHost::packageName)
            .toSet()
        return platform.personalAppLabels(packages)
    }

    /** Asks whether root is granted, as the root mode needs; said so if it is not, when asked for. */
    fun checkRoot() {
        viewModelScope.launch {
            if (userData.userData.first().rootSwitchPending) refresh() else readRoot(reportMissing = true)
        }
    }

    private suspend fun readRoot(reportMissing: Boolean) {
        if (service.value.checkingRoot) return
        service.update { it.copy(checkingRoot = true) }
        try {
            val available = suspendRunCatching { methods.rootAvailable() }.getOrDefault(false)
            service.update { it.copy(rootAvailable = available) }
            if (!available && reportMissing) messages.show(UiMessage(R.string.feature_settings_root_missing))
        } finally {
            service.update { it.copy(checkingRoot = false) }
        }
    }

    /**
     * Requests root filtering. The module takes over once it is ready
     * and has the VPN's settings.
     */
    fun switchToRoot() = busy {
        val switched = methods.switchTo(FilterMethod.Root) == FilterMethodSwitch.Complete
        messages.show(UiMessage(if (switched) R.string.feature_settings_switched_to_root else R.string.feature_settings_switch_root_pending))
    }

    fun cancelRootSwitch() = busy {
        methods.cancelRootSwitch()
    }

    /** Turns URL filtering off before the certificate is removed in the system settings: every app is then filtered by domain. */
    fun turnOffUrlFiltering() = busy {
        daemon.setUrlFiltering(false)
        https.reload()
    }

    fun setThemeMode(mode: ThemeMode) = launch { userData.setThemeMode(mode) }

    fun setUseDynamicColor(use: Boolean) = launch { userData.setUseDynamicColor(use) }

    fun setUpdateInterval(interval: UpdateInterval) = launch { userData.setUpdateInterval(interval) }

    fun setUpdateOnWifiOnly(wifiOnly: Boolean) = launch { userData.setUpdateOnWifiOnly(wifiOnly) }

    fun setFilteringScope(scope: FilteringScope) = busy { daemon.setFilteringScope(scope) }

    fun setShowSystemApps(show: Boolean) = launch { userData.setShowSystemApps(show) }

    /** Off, nothing is decrypted: every app is filtered by domain only. */
    fun setUrlFiltering(on: Boolean) = changeHttps(
        on,
        DaemonStatus::urlFiltering,
        { copy(urlFiltering = it) },
        onSaved = { messages.show(UiMessage(if (it) R.string.feature_settings_url_filtering_on else R.string.feature_settings_url_filtering_off)) },
        write = daemon::setUrlFiltering,
    )

    /** On, the elements the lists name are hidden on decrypted pages (SECURITY.md says what that changes). */
    fun setCosmetic(on: Boolean) = changeHttps(on, DaemonStatus::cosmetic, { copy(cosmetic = it) }, write = daemon::setCosmetic)

    private fun changeHttps(
        on: Boolean,
        current: KProperty1<DaemonStatus, Boolean?>,
        change: DaemonStatus.(Boolean) -> DaemonStatus,
        onSaved: (Boolean) -> Unit = {},
        write: suspend (Boolean) -> Unit,
    ) {
        val status = service.value.status.valueOrNull ?: return
        val before = current(status) ?: return
        if (before == on || service.value.busy) return
        val alreadySaving = changingHttps.put(current, on) != null
        service.update { it.copy(status = it.status.map { value -> value.change(on) }) }
        if (alreadySaving) return
        viewModelScope.launch {
            try {
                var saved = before
                var requested = on
                while (true) {
                    val result = suspendRunCatching("https settings") { write(requested) }
                        .onSuccess { saved = requested }
                    val next = changingHttps.getValue(current)
                    if (next != requested) {
                        requested = next
                        continue
                    }
                    result.onSuccess { onSaved(saved) }.onFailure { e ->
                        service.update { it.copy(status = it.status.map { value -> value.change(saved) }) }
                        showWriteFailure(e)
                    }
                    break
                }
            } finally {
                httpsRevision++
                changingHttps.remove(current)
                refresh()
            }
        }
    }

    /** Restarts the daemon through the module's service script, which stops the running one first. */
    fun restartService() = busy {
        if (module.startDaemon()) messages.show(UiMessage(R.string.feature_settings_restarted)) else messages.show(UiMessage(R.string.feature_settings_module_disabled))
    }

    /** Installs or updates the module this app carries. */
    fun installModule() = busy {
        val active = module.installBundled()
        val pending = userData.userData.first().rootSwitchPending
        val message = when {
            !active -> R.string.feature_settings_module_restart
            pending -> if (methods.retryRootSwitch()) R.string.feature_settings_switched_to_root else R.string.feature_settings_switch_root_pending
            else -> R.string.feature_settings_module_installed
        }
        messages.show(UiMessage(message))
    }

    /** Has an app try our certificate again for [host], or for every host it pinned if null. */
    fun forgetPinned(packageName: String, host: String?) = busy {
        apps.forgetPinned(packageName, host)
        messages.show(UiMessage(R.string.feature_settings_pinned_forgotten))
    }

    fun forgetAllPinned() = busy {
        val pinned = service.value.pinned.valueOrNull?.takeIf { it.isNotEmpty() } ?: return@busy
        pinned.map(PinnedHost::appKey).distinct().forEach { apps.forgetPinned(it, null) }
        messages.show(UiMessage(R.string.feature_settings_pinned_forgotten))
    }

    fun exportLogs(uri: Uri) = busy {
        val message = when (diagnostics.exportLogs(uri)) {
            LogExportResult.Complete -> R.string.feature_settings_logs_exported
            LogExportResult.DaemonIncomplete -> R.string.feature_settings_logs_exported_partial
        }
        messages.show(UiMessage(message))
    }

    private fun launch(block: suspend () -> Unit) {
        viewModelScope.launchCatching("settings", onFailure = {
            messages.show(UiMessage(R.string.feature_settings_failed))
        }) {
            block()
        }
    }

    private fun busy(block: suspend () -> Unit) {
        viewModelScope.launch {
            service.update { it.copy(busy = true) }
            try {
                suspendRunCatching("settings") {
                    block()
                }.onFailure(::showWriteFailure)
            } finally {
                service.update { it.copy(busy = false) }
                refresh()
            }
        }
    }

    private fun showWriteFailure(e: Throwable) {
        messages.show(UiMessage(if (e is AbdException.TimedOut) R.string.feature_settings_service_timeout else R.string.feature_settings_failed))
    }
}
