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

package li.mercury.adblocker.feature.onboarding.impl

import android.content.Intent
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DeviceRepository
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.DeviceCheck
import li.mercury.adblocker.core.model.DeviceCheckKind
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.WorkSelection
import timber.log.Timber
import javax.inject.Inject

/**
 * The steps, the same six for either method. [About] says what the method
 * costs: the certificate under root, the VPN without. [Install] sets up and
 * starts filtering: sets up root mode, or starts the VPN.
 */
enum class OnboardingStep { Welcome, Check, About, Apps, Lists, Install }

/** Where the installation is. */
sealed interface InstallState {
    data object Idle : InstallState

    data class Running(val stage: InstallStage, val progress: Float) : InstallState

    /**
     * Done; [active] when filtering runs, false when the VPN was set up with
     * no app to filter yet. With the VPN, [rules] is how many the downloaded
     * lists hold. [listsFailed] says whether the download failed, to be
     * retried in the background.
     */
    data class Done(val active: Boolean, val apps: Int, val rules: Int? = null, val listsFailed: Boolean = false) : InstallState

    /**
     * Not done, for [failure]. [detail] is what the installer said was wrong
     * when it was the one to fail, for a line under the app's own message;
     * null otherwise.
     */
    data class Failed(val failure: LoadFailure, val detail: String? = null) : InstallState
}

/** Setting up root mode, or the VPN's start, which only the VPN has. */
enum class InstallStage { Module, Apps, Vpn, Lists }

data class OnboardingUiState(
    val step: OnboardingStep = OnboardingStep.Welcome,
    /** The checks done so far; a kind not in it is still being checked. */
    val checks: Map<DeviceCheckKind, DeviceCheck> = emptyMap(),
    val acknowledged: Boolean = false,
    val apps: List<InstalledApp> = emptyList(),
    val workInventoryUnavailable: Boolean = true,
    val personalEligibilityUnavailable: Boolean = false,
    val picks: Set<String> = emptySet(),
    val filteringScope: FilteringScope = FilteringScope.Chosen,
    /** Waiting for the installed app list. */
    val appsLoading: Boolean = false,
    /** The default lists, and whether each is kept. */
    val lists: Map<String, Boolean> = FilterCatalog.setupLists(emptyList()),
    val install: InstallState = InstallState.Idle,
    /** With the VPN: whether notifications may be shown, once asked; null before. */
    val notifications: Boolean? = null,
    /** With the VPN: the system's consent was refused the last time it was asked. */
    val consentRefused: Boolean = false,
    val vpnTransferIncomplete: Boolean = false,
    /** The method setup goes on with, set when the check is left: root mode where it can run, the VPN otherwise. */
    val method: FilterMethod = FilterMethod.Root,
) {
    val previousStep: OnboardingStep?
        get() = if (step == OnboardingStep.Welcome || install is InstallState.Running ||
            (install as? InstallState.Done)?.active == true
        ) {
            null
        } else {
            OnboardingStep.entries[step.ordinal - 1]
        }

    val willStartVpn: Boolean get() = filteringScope == FilteringScope.All || picks.isNotEmpty()

    val setupAppCount: Int get() = if (filteringScope == FilteringScope.All) {
        apps.count { it.profileSerial == null && it.canEnableFiltering } + picks.count { ':' in it }
    } else {
        picks.size
    }

    val checksPassed: Boolean get() = checks.size == DeviceCheckKind.entries.size && checks.values.all { it.passed }
    val checking: Boolean get() = checks.size < DeviceCheckKind.entries.size

    /** Root is granted; whether root mode can run is [checksPassed]. */
    val rootGranted: Boolean get() = checks[DeviceCheckKind.Root]?.passed == true

    /**
     * Whether the VPN can run here: its engine is built for this processor.
     * It needs Android 10, the app's own minimum, and no root.
     */
    val vpnPossible: Boolean get() = checks[DeviceCheckKind.Architecture]?.passed == true

    /** Whether setup can go on from the check, with either method. */
    val canContinue: Boolean get() = !checking && (checksPassed || vpnPossible)
}

@HiltViewModel
class OnboardingViewModel @Inject constructor(
    private val device: DeviceRepository,
    private val module: ModuleRepository,
    private val apps: AppsRepository,
    private val lists: ListsRepository,
    private val userData: UserDataRepository,
    private val scheduler: ListUpdateScheduler,
    private val methods: FilterMethodRepository,
    private val vpn: VpnController,
    private val daemon: DaemonRepository,
) : ViewModel() {
    private val state = MutableStateFlow(OnboardingUiState(lists = FilterCatalog.setupLists(device.languageTags())))
    val uiState: StateFlow<OnboardingUiState> = state.asStateFlow()

    private var checking: Job? = null
    private var loadingApps: Job? = null
    private val rememberedPicks = mutableSetOf<String>()
    private var vpnSetup: Job? = null

    /** Whether the lists still have to be downloaded once onboarding is left: the download failed. */
    private var downloadPending = true

    fun next() {
        val step = state.value.step
        if (step == OnboardingStep.Install) return
        if (step == OnboardingStep.Apps && (state.value.appsLoading || state.value.personalEligibilityUnavailable)) return
        if (step == OnboardingStep.Check) {
            if (!state.value.canContinue) return
            state.update { it.copy(method = if (it.checksPassed) FilterMethod.Root else FilterMethod.Vpn) }
        }
        go(OnboardingStep.entries[step.ordinal + 1])
    }

    fun back() {
        state.value.previousStep?.let(::go)
    }

    private fun go(step: OnboardingStep) {
        state.update { it.copy(step = step) }
        when (step) {
            OnboardingStep.Check -> if (!state.value.checksPassed) runChecks()

            OnboardingStep.Apps -> loadApps()

            // The VPN starts when asked: the system's dialogs follow the tap.
            OnboardingStep.Install -> if (state.value.method == FilterMethod.Root) install()

            else -> Unit
        }
    }

    /**
     * Checks one thing at a time, root first: it is what asks the user to
     * grant access. Checks still under way are dropped, so that two runs
     * never fill in the same list.
     */
    fun runChecks() {
        checking?.cancel()
        state.update { it.copy(checks = emptyMap()) }
        checking = viewModelScope.launch {
            for (kind in DeviceCheckKind.entries) {
                val check = suspendRunCatching { device.check(kind) }.getOrElse { DeviceCheck(kind, passed = false, detail = null) }
                state.update { it.copy(checks = it.checks + (kind to check)) }
            }
        }
    }

    fun setFilteringScope(scope: FilteringScope) = state.update { it.copy(filteringScope = scope) }

    fun acknowledge(value: Boolean) = state.update { it.copy(acknowledged = value) }

    fun loadApps() {
        loadingApps?.cancel()
        val method = state.value.method
        state.update { it.copy(appsLoading = true) }
        loadingApps = viewModelScope.launch {
            val inventory = suspendRunCatching { apps.installed(method) }
                .getOrElse {
                    AppInventory(emptyList(), workUnavailable = method == FilterMethod.Root, personalUnavailable = method == FilterMethod.Root)
                }
            val sorted = inventory.apps.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER, InstalledApp::label))
            val available = sorted.filter(InstalledApp::canEnableFiltering).map(InstalledApp::appKey).toSet()
            state.update {
                it.copy(
                    apps = sorted,
                    appsLoading = false,
                    workInventoryUnavailable = inventory.workUnavailable,
                    personalEligibilityUnavailable = inventory.personalUnavailable,
                    picks = rememberedPicks intersect available,
                )
            }
        }
    }

    fun togglePick(packageName: String) {
        val current = state.value
        if (current.appsLoading || current.apps.none { it.appKey == packageName && it.canEnableFiltering }) return
        if (!rememberedPicks.add(packageName)) rememberedPicks.remove(packageName)
        state.update {
            it.copy(picks = if (packageName in rememberedPicks) it.picks + packageName else it.picks - packageName)
        }
    }

    fun toggleList(url: String) = state.update {
        val enabled = it.lists[url] ?: return@update it
        it.copy(lists = it.lists + (url to !enabled))
    }

    /**
     * Sets up root mode where it is not ready, then chooses the apps and
     * downloads the lists. A module flashed in Magisk or APatch that waits for
     * a restart is set up the same way: the app's own filters now, and the
     * manager's takes over at the restart, with the same settings.
     */
    fun install() {
        val choice = state.value
        if (choice.install is InstallState.Running) return
        viewModelScope.launch {
            downloadPending = true
            suspendRunCatching("install") {
                state.update { it.copy(install = InstallState.Running(InstallStage.Module, 0.1f)) }
                // Only after a VPN setup that did not finish, before root was granted.
                val rootSelected = userData.userData.first().method == FilterMethod.Root || methods.switchTo(FilterMethod.Root) == FilterMethodSwitch.Complete
                when (module.state()) {
                    // Ready says what is installed, not that it runs: nothing
                    // may have started it since the device or the app did.
                    // A module that needs a restart is in use too, and the
                    // installer leaves it to the root manager until then;
                    // an older one rejects what follows, which says so.
                    ModuleState.Ready, ModuleState.NeedsRestart -> module.ensureRunning()

                    else -> if (!setUpRootMode()) return@suspendRunCatching
                }
                if (!rootSelected && !methods.retryRootSwitch() && userData.userData.first().method != FilterMethod.Root) {
                    throw AbdException.CommandFailed("Root setup is waiting for the module")
                }
                state.update { it.copy(install = InstallState.Running(InstallStage.Apps, 0.3f)) }
                lists.configure(choice.lists)
                apps.configure(choice.picks, work = if (choice.workInventoryUnavailable) WorkSelection.Keep else WorkSelection.Replace)
                daemon.setFilteringScope(choice.filteringScope)
                userData.setPending(emptySet(), emptySet(), emptySet())
                downloadPending = !download()
                state.update { it.copy(install = InstallState.Done(active = true, apps = choice.setupAppCount, listsFailed = downloadPending)) }
            }.onFailure { e ->
                state.update { it.copy(install = InstallState.Failed(e.toLoadFailure())) }
            }
        }
    }

    /** Downloads the enabled lists, from 40 % of the progress on; whether every one came. */
    private suspend fun download(): Boolean {
        state.update { it.copy(install = InstallState.Running(InstallStage.Lists, 0.4f)) }
        var total = 0
        var done = 0
        var failed = true
        lists.update().collect { event ->
            when (event) {
                is UpdateEvent.Started -> total = event.lists
                is UpdateEvent.ListProgress -> if (event.state != ListUpdateState.Fetching) done++
                is UpdateEvent.Finished -> failed = event.hasFailures
            }
            val progress = if (total == 0) 0.4f else 0.4f + 0.6f * done / total
            state.update { it.copy(install = InstallState.Running(InstallStage.Lists, progress)) }
        }
        return !failed
    }

    /** The system's consent screen to show before the VPN starts, or null once given. */
    fun consentIntent(): Intent? = vpn.consentIntent()

    /** Whether notifications may be shown, as the system answered or had it. */
    fun notificationsAnswered(allowed: Boolean) = state.update { it.copy(notifications = allowed) }

    /** The VPN was not allowed: the start step again, saying so. */
    fun consentRefused() = state.update { it.copy(consentRefused = true, install = InstallState.Idle) }

    /**
     * With consent given: stores the method before anything is written for
     * it, and saves the selected apps and lists. When apps were selected,
     * starts the VPN and downloads the lists. A failed download is retried
     * in the background.
     */
    fun startVpn() {
        val choice = state.value
        if (choice.install is InstallState.Running || choice.install is InstallState.Done || vpnSetup?.isActive == true) return
        vpnSetup = viewModelScope.launch {
            downloadPending = true
            suspendRunCatching("vpn start") {
                state.update { it.copy(consentRefused = false, install = InstallState.Running(InstallStage.Apps, 0.1f)) }
                if (methods.switchTo(FilterMethod.Vpn) == FilterMethodSwitch.VpnTransferIncomplete) {
                    state.update { it.copy(vpnTransferIncomplete = true) }
                }
                lists.configure(choice.lists)
                apps.configure(choice.picks)
                daemon.setFilteringScope(choice.filteringScope)
                userData.setPending(emptySet(), emptySet(), emptySet())
                if (!choice.willStartVpn) {
                    state.update { it.copy(install = InstallState.Done(active = false, apps = 0)) }
                    return@suspendRunCatching
                }
                state.update { it.copy(install = InstallState.Running(InstallStage.Vpn, 0.3f)) }
                vpn.start()
                val started = withTimeoutOrNull(VPN_START_TIMEOUT_MS) { vpn.status.first { it.state != VpnState.Starting } }
                    ?: throw AbdException.TimedOut()
                if (started.state != VpnState.On) throw AbdException.CommandFailed("VPN did not start")

                // Keep watching until onboarding is left. If the service
                // stops meanwhile, cancel any download and let setup retry.
                coroutineScope {
                    launch {
                        val downloaded = download()
                        downloadPending = !downloaded
                        val rules = suspendRunCatching { daemon.status().rules }.getOrNull()
                        if (vpn.status.value.state == VpnState.On) {
                            state.update {
                                it.copy(install = InstallState.Done(active = true, apps = choice.setupAppCount, rules = rules, listsFailed = !downloaded))
                            }
                        }
                    }
                    vpn.status.first { it.state != VpnState.On }
                    throw AbdException.CommandFailed("VPN stopped during setup")
                }
            }.onFailure { e ->
                state.update { it.copy(install = InstallState.Failed(e.toLoadFailure())) }
            }
        }
    }

    /**
     * Sets up root mode and returns whether it runs; false when the installer
     * failed, which is then the state, with what it said was wrong.
     */
    private suspend fun setUpRootMode(): Boolean = try {
        module.installBundled()
    } catch (e: AbdException.CommandFailed) {
        Timber.w("install: installer failed")
        state.update { it.copy(install = InstallState.Failed(LoadFailure.Failed, detail = e.message?.trim()?.ifEmpty { null })) }
        false
    }

    /** Leaves onboarding for the app. */
    fun finish() {
        viewModelScope.launch {
            val choice = state.value
            val installed = choice.install as? InstallState.Done ?: return@launch
            if (choice.method == FilterMethod.Vpn && installed.active && vpn.status.value.state != VpnState.On) return@launch
            userData.setOnboardingDone(true)
            vpnSetup?.cancel()
            // A download that failed is done in the background; one that went
            // well is not done again.
            if (downloadPending) scheduler.updateSoon()
        }
    }

    private companion object {
        const val VPN_START_TIMEOUT_MS = 10_000L
    }
}
