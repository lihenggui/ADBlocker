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
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
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
import li.mercury.adblocker.core.data.repository.ModuleUpdate
import li.mercury.adblocker.core.data.repository.ModuleUpdater
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

/** Work the page does on root mode: under way, or failed the last time it was tried, until it is tried again. */
enum class RootTask { Idle, Running, Failed }

/** What the settings page shows of the daemon and of root mode. */
data class ServiceInfo(
    val status: ReadState<DaemonStatus> = ReadState.Pending,
    val module: ModuleState? = null,
    /** Whether the app made the installation in use, which it can then remove; null when unknown or none is in use. */
    val installedByApp: Boolean? = null,
    /** A module flashed in a root manager waits for the next restart to take over. */
    val stagedInManager: Boolean = false,
    val rootManager: String? = null,
    val setup: RootTask = RootTask.Idle,
    val removal: RootTask = RootTask.Idle,
    /** Switching from the VPN to root mode, which sets root mode up where it has to. */
    val switchingToRoot: Boolean = false,
    /** Under root, with root mode not set up: whether this build can filter with the VPN instead. */
    val vpnAvailable: Boolean = false,
    val pinned: ReadState<List<PinnedHost>> = ReadState.Pending,
    val busy: Boolean = false,
    /** Without root: whether the certificate is in the user store. */
    val certificateInstalled: Boolean = false,
    /** Without root: whether root is granted, so that root mode can be chosen; null until asked. */
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
    /** Bringing root mode up to date with this app, asked for here or done when the app was opened. */
    val update: ModuleUpdate = ModuleUpdate.Idle,
)

@HiltViewModel
class SettingsViewModel @Inject constructor(
    private val platform: AppPlatformInfo,
    private val userData: UserDataRepository,
    private val daemon: DaemonRepository,
    private val module: ModuleRepository,
    private val updater: ModuleUpdater,
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
    val uiState: StateFlow<SettingsUiState> = combine(userData.userData, service, vpn.status, appVersion, updater.state) { data, info, vpnStatus, version, update ->
        SettingsUiState(data, info, version, vpnStatus, update)
    }
        // An update, asked for here or not, changes what is installed.
        .refreshWhileCollected(merge(changes.changes, reload, updater.state.drop(1).map {}), ::load)
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
        val installation = readInstallation()
        val status = readSection { daemon.status() }
        val rootManager = suspendRunCatching { module.rootManager() }.getOrNull()
        val vpnAvailable = installation.module == ModuleState.NotInstalled && suspendRunCatching { vpn.engineAvailable() }.getOrDefault(false)
        val pinned = readSection { apps.pinned() }
        val labels = pinned.valueOrNull?.let { pinnedLabels(it) }.orEmpty()
        service.update {
            installation.applyTo(it).copy(
                status = statusAfterRead(it.status, status, revision),
                rootManager = rootManager,
                vpnAvailable = vpnAvailable,
                pinned = pinned,
                pinnedLabels = labels,
            )
        }
    }

    /** Root mode's state, whose installation is in use, and whether a module waits in a root manager. */
    private class Installation(val module: ModuleState, val installedByApp: Boolean?, val staged: Boolean) {
        fun applyTo(info: ServiceInfo) = info.copy(module = module, installedByApp = installedByApp, stagedInManager = staged)
    }

    private suspend fun readInstallation(): Installation {
        val state = readModule()
        // Only an installation in use has an origin.
        val inUse = state == ModuleState.Ready || state == ModuleState.Mismatched
        val byApp = if (inUse) suspendRunCatching { module.isInstalledByApp() }.getOrNull() else null
        // What waits replaces the app's installation at the restart, or is all there is until then.
        val mayWait = byApp == true || state == ModuleState.NotInstalled
        val staged = state == ModuleState.NeedsRestart || (mayWait && suspendRunCatching { module.isStagedInManager() }.getOrDefault(false))
        return Installation(state, byApp, staged)
    }

    /**
     * Without root: the engine's state, the certificate and the
     * notifications. Nothing here runs a root command, which would show the
     * root manager's prompt; root is asked for once, and when asked again.
     */
    private suspend fun readVpn(revision: Long) {
        // Asked first, so that where root is granted an installation is read with the rest.
        if (service.value.rootAvailable == null) readRoot(reportMissing = false)
        val status = readSection { daemon.status() }
        val pinned = readSection { apps.pinned() }
        val labels = pinned.valueOrNull?.let { pinnedLabels(it) }.orEmpty()
        val installed = withContext(io) { https.certificateInstalled() }
        val notifications = platform.notificationsAllowed()
        // An installation left by a cancelled switch is removed from here.
        val installation = if (userData.userData.first().rootSwitchPending || service.value.rootAvailable == true) readInstallation() else null
        val rootManager = if (installation?.installedByApp == false) suspendRunCatching { module.rootManager() }.getOrNull() else null
        service.update {
            (installation?.applyTo(it) ?: it.copy(module = null, installedByApp = null, stagedInManager = false)).copy(
                status = statusAfterRead(it.status, status, revision),
                rootManager = rootManager,
                pinned = pinned,
                certificateInstalled = installed,
                notificationsAllowed = notifications,
                pinnedLabels = labels,
            )
        }
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
     * Switches from the VPN to root mode in one go: sets root mode up where
     * nothing usable is installed, then has it take over with the VPN's
     * settings. The VPN filters until it has.
     */
    fun switchToRoot() {
        if (service.value.busy) return
        viewModelScope.launch {
            service.update { it.copy(busy = true, switchingToRoot = true) }
            try {
                val switched = suspendRunCatching("root switch") { setUpAndSwitch() }.getOrDefault(false)
                messages.show(UiMessage(if (switched) R.string.feature_settings_switched_to_root else R.string.feature_settings_switch_root_pending))
            } finally {
                service.update { it.copy(busy = false, switchingToRoot = false) }
                refresh()
            }
        }
    }

    private suspend fun setUpAndSwitch(): Boolean {
        if (methods.switchTo(FilterMethod.Root) == FilterMethodSwitch.Complete) return true
        if (readModule() !in NEEDS_SETUP) return false
        module.installBundled()
        return methods.retryRootSwitch()
    }

    /** With root mode not set up: filtering moves to the VPN, which the overview then connects. */
    fun switchToVpn() = busy {
        methods.switchTo(FilterMethod.Vpn)
        messages.show(UiMessage(R.string.feature_settings_switched_to_vpn))
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
        if (module.startDaemon()) messages.show(UiMessage(R.string.feature_settings_restarted)) else showTurnedOffInManager()
    }

    /** Turns root mode on again after two startups in a row did not finish, and starts it. */
    fun turnOnAgain() = busy {
        if (!module.turnOnAgain()) showTurnedOffInManager()
    }

    /** Only a module in a root manager stays off: the app turns its own installation on again. */
    private suspend fun showTurnedOffInManager() {
        val manager = service.value.rootManager ?: suspendRunCatching { module.rootManager() }.getOrNull()
        messages.show(
            if (manager == null) UiMessage(R.string.feature_settings_module_disabled) else UiMessage(R.string.feature_settings_module_disabled_in, listOf(manager)),
        )
    }

    /** Sets up root mode with the app's own copy, which runs at once. */
    fun setUpRootMode() = rootTask({ copy(setup = it) }) {
        module.installBundled()
        messages.show(UiMessage(R.string.feature_settings_module_installed))
    }

    /** Replaces the installed root mode with this app's; [SettingsUiState.update] says how it goes. */
    fun updateRootMode() {
        if (service.value.busy) return
        viewModelScope.launch {
            service.update { it.copy(busy = true) }
            try {
                updater.update()
                if (updater.state.value == ModuleUpdate.Idle) messages.show(UiMessage(R.string.feature_settings_root_updated))
            } finally {
                service.update { it.copy(busy = false) }
                refresh()
            }
        }
    }

    /** Removes the installation the app made, with what it kept. */
    fun removeRootMode() = rootTask({ copy(removal = it) }) {
        module.remove()
        messages.show(UiMessage(R.string.feature_settings_root_removed))
    }

    /**
     * Runs [work] on root mode, its row saying that it runs and then whether
     * it failed. Root mode is read back before the row says how it went, so
     * that the row and the installation it describes agree.
     */
    private fun rootTask(mark: ServiceInfo.(RootTask) -> ServiceInfo, work: suspend () -> Unit) {
        if (service.value.busy) return
        viewModelScope.launch {
            service.update { it.copy(busy = true).mark(RootTask.Running) }
            try {
                val outcome = if (suspendRunCatching("root mode") { work() }.isSuccess) RootTask.Idle else RootTask.Failed
                val installation = readInstallation()
                service.update { installation.applyTo(it).mark(outcome) }
            } finally {
                service.update { it.copy(busy = false) }
                refresh()
            }
        }
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

    private companion object {
        /**
         * What a switch to root mode sets up first: nothing usable of this
         * app's is in use. Not a module that needs a restart, which the
         * installer leaves to the root manager.
         */
        val NEEDS_SETUP = setOf(ModuleState.NotInstalled, ModuleState.Mismatched)
    }
}
