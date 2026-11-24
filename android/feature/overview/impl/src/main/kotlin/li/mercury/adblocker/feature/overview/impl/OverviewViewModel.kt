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

package li.mercury.adblocker.feature.overview.impl

import android.content.Intent
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.DEFAULT
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.result.launchCatching
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.ModuleUpdate
import li.mercury.adblocker.core.data.repository.ModuleUpdater
import li.mercury.adblocker.core.data.repository.OverviewRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.ui.MessageQueue
import li.mercury.adblocker.core.ui.UiMessage
import java.time.Instant
import javax.inject.Inject

sealed interface OverviewUiState {
    data object Loading : OverviewUiState

    data class Failed(val failure: LoadFailure) : OverviewUiState

    /** Root mode is being brought up to date with this app, and cannot be read until it is. */
    data object Updating : OverviewUiState

    /**
     * An older module in a root manager rejects what this app asks, and the
     * manager replaces it only at the next restart: nothing in the app can.
     */
    data object RestartNeeded : OverviewUiState

    data class Ready(
        val status: DaemonStatus,
        val filteredApps: Int,
        val stats: StatsSummary,
        /** The apps that blocked the most today, with their counts. */
        val topApps: List<Pair<AppEntry, Long>>,
        val enabledLists: Int,
        val listsUpdated: Instant?,
        val method: FilterMethod = FilterMethod.Root,
        /** With the VPN: where it is, which says whether filtering runs. */
        val vpn: VpnStatus = VpnStatus(),
        /** With the VPN: how far HTTPS inspection is set up. */
        val https: HttpsSetup? = null,
        /** On lists without a copy or whose download from a new address failed. */
        val listDownloads: ListDownloads? = null,
        /** What to know about now: a state shown while it holds, or a tip shown until dismissed. */
        val notice: OverviewNotice? = null,
        /** Root mode is being brought up to date with this app; the old one filters meanwhile. */
        val updating: Boolean = false,
    ) : OverviewUiState
}

data class ListDownloads(val missing: Int, val missingFailed: Boolean, val usingPreviousAddress: Int = 0) {
    val failed: Boolean get() = missingFailed || usingPreviousAddress > 0
}

/** HTTPS without root: the installed certificate, the URL filtering setting, and the apps using HTTPS inspection. */
data class HttpsSetup(val certificateInstalled: Boolean, val urlFiltering: Boolean, val decryptedApps: Int)

enum class OverviewNotice {
    /**
     * With the VPN in All apps, the system withholds the installed app list:
     * the VPN routes only the few apps it shows. A state, as [ListDownloads]
     * is; it goes once the list is allowed.
     */
    AppListWithheld,

    /**
     * Lists are waiting for a first download or are still using a previous
     * copy after a failed download from a new address.
     */
    ListDownloads,

    /** The system's "Block connections without VPN" is on: the chosen apps have no network while the VPN is down. */
    Lockdown,

    /** Root was granted since the VPN was chosen: root mode can filter instead. */
    RootAvailable,

    BatteryExemption,
}

/** How long the overview waits between reads while it is shown: the daemon writes its counts every 30 s at most. */
private const val REFRESH_MS = 30_000L

@OptIn(ExperimentalCoroutinesApi::class)
@HiltViewModel
class OverviewViewModel @Inject constructor(
    private val daemon: DaemonRepository,
    private val apps: AppsRepository,
    private val overview: OverviewRepository,
    private val module: ModuleRepository,
    private val updater: ModuleUpdater,
    private val userData: UserDataRepository,
    private val vpn: VpnController,
    private val methods: FilterMethodRepository,
    private val https: HttpsRepository,
    private val lists: ListsRepository,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
    @Dispatcher(DEFAULT) private val defaultDispatcher: CoroutineDispatcher,
    changes: DataChanges,
) : ViewModel() {
    private val messages = MessageQueue()
    val pendingMessages = messages.pending

    fun onMessageShown(id: Long) = messages.onMessageShown(id)
    private val reload = MutableSharedFlow<Unit>(extraBufferCapacity = 1)

    /**
     * The installed apps, as last read. Reading them means asking the package
     * manager about every app, with its name: done on start, on request and
     * after a change, not at every tick.
     */
    private var installed: AppInventory<AppEntry>? = null

    /**
     * Set by every trigger that asks for the apps and cleared only once they
     * were read: a tick that cancels a load in flight must not leave the
     * replacement with the cache the change made stale.
     */
    private var appsStale = true

    /** Whether root is granted, asked once while the VPN is in use: asking can show the root manager's prompt. */
    private var rootAvailable = false
    private var rootProbeStarted = false

    private var download: Job? = null

    // Read on start, after a change made anywhere in the app, on request, and
    // every REFRESH_MS while the screen is shown: nothing runs once it is not.
    val uiState: StateFlow<OverviewUiState> = merge(
        flow {
            emit(true)
            while (true) {
                delay(REFRESH_MS)
                emit(false)
            }
        },
        changes.changes.map { true },
        reload.map { true },
        // The VPN started, stopped or was taken over: the engine's status changes with it.
        vpn.status.drop(1).map { false },
        // Root mode began or finished updating: what runs changes with it.
        updater.state.drop(1).map { false },
        // An update started or ended, wherever it was asked: the lists notice waits for it.
        lists.updating.drop(1).map { false },
    ).onEach { withApps -> if (withApps) appsStale = true }
        .mapLatest { load() }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), OverviewUiState.Loading)

    fun onStart() {
        refresh()
        // Opening this screen may ask the root manager for a grant. Observing state cannot do that.
        if (rootProbeStarted) return
        rootProbeStarted = true
        viewModelScope.launch {
            val data = userData.userData.first()
            if (!data.capabilities.usesVpn || ROOT_TIP in data.dismissedTips) return@launch
            rootAvailable = suspendRunCatching { methods.rootAvailable() }.getOrDefault(false)
            if (rootAvailable) refresh()
        }
    }

    private suspend fun load(): OverviewUiState {
        val updating = updater.state.value == ModuleUpdate.Updating
        // While root mode updates, a failed read is the update's doing: it is said once the update ends.
        return read(updating).getOrElse { e ->
            val failure = e.toLoadFailure()
            when {
                updating -> OverviewUiState.Updating

                // Settings has no update to offer for it, so the failure's own text would send the user there for nothing.
                failure == LoadFailure.ModuleOutdated && suspendRunCatching { module.state() }.getOrNull() == ModuleState.NeedsRestart -> OverviewUiState.RestartNeeded

                else -> OverviewUiState.Failed(failure)
            }
        }
    }

    private suspend fun read(updating: Boolean): Result<OverviewUiState> = suspendRunCatching("overview") {
        val (status, summary, subscriptions) = overview.snapshot()
        val inventory = installed.takeUnless { appsStale }
            ?: withContext(defaultDispatcher) { apps.inventory() }.also {
                installed = it
                appsStale = false
            }
        val entries = inventory.apps
        val data = userData.userData.first()
        val dismissed = data.dismissedTips
        val vpnStatus = vpn.status.value
        val downloads = listDownloads(subscriptions, data)
        // Apps the VPN cannot see come first, since no list can filter them.
        val notice = when {
            appsUnseen(data, status, inventory) -> OverviewNotice.AppListWithheld
            downloads != null && !lists.updating.value -> OverviewNotice.ListDownloads
            !data.capabilities.usesVpn -> null
            vpnStatus.lockdown && LOCKDOWN_TIP !in dismissed -> OverviewNotice.Lockdown
            withContext(io) { vpn.shouldOfferBatteryExemption() } -> OverviewNotice.BatteryExemption
            rootAvailable && !data.rootSwitchPending && ROOT_TIP !in dismissed -> OverviewNotice.RootAvailable
            else -> null
        }
        withContext(defaultDispatcher) {
            val filtered = entries.filter { it.isFiltered }
            OverviewUiState.Ready(
                status = status,
                filteredApps = status.filteringApps ?: filtered.size,
                stats = summary,
                topApps = filtered
                    .map { it to (summary.perApp[it.appKey]?.today?.total ?: 0L) }
                    .filter { it.second > 0 }
                    .sortedByDescending { it.second }
                    .take(TOP_APPS),
                enabledLists = subscriptions.count { it.enabled },
                listsUpdated = status.listsUpdated,
                method = data.method,
                vpn = vpnStatus,
                https = if (data.capabilities.needsUserCa) {
                    HttpsSetup(
                        certificateInstalled = withContext(io) { https.certificateInstalled() },
                        urlFiltering = status.urlFiltering == true,
                        decryptedApps = filtered.count { it.filter?.level == FilterLevel.Full },
                    )
                } else {
                    null
                },
                listDownloads = downloads,
                notice = notice,
                updating = updating,
            )
        }
    }

    /** Reads everything again; the screen asks each time it is shown. */
    fun refresh() {
        reload.tryEmit(Unit)
    }

    /** Pauses every app for [minutes], or until resumed if null. */
    fun pause(minutes: Int?) = act {
        daemon.pause(minutes)
        val undo = R.string.feature_overview_undo
        val message = when {
            minutes == null -> UiMessage(R.string.feature_overview_paused_until_resumed, actionLabel = undo) { resume() }
            minutes % 60 == 0 -> UiMessage(R.plurals.feature_overview_paused_for_hours, listOf(minutes / 60), undo, quantity = minutes / 60) { resume() }
            else -> UiMessage(R.plurals.feature_overview_paused_for_minutes, listOf(minutes), undo, quantity = minutes) { resume() }
        }
        messages.show(message)
    }

    fun resume() = act { daemon.resume() }

    /** Starts the daemon, which takes a moment; the overview is read again once it runs, or once it stopped waiting. */
    fun startService() = act {
        try {
            if (!module.startDaemon()) showTurnedOffInManager()
        } finally {
            refresh()
        }
    }

    /** Turns root mode on again after its boot guard turned it off, and starts the daemon. */
    fun turnOnAgain() = act {
        try {
            if (!module.turnOnAgain()) showTurnedOffInManager()
        } finally {
            refresh()
        }
    }

    /** Only a module in a root manager stays off: the app turns its own installation on again. */
    private suspend fun showTurnedOffInManager() {
        val manager = suspendRunCatching { module.rootManager() }.getOrNull()
        messages.show(
            if (manager == null) UiMessage(R.string.feature_overview_module_disabled) else UiMessage(R.string.feature_overview_module_disabled_in, listOf(manager)),
        )
    }

    /** The system's consent screen to show before the VPN starts, or null once given. */
    fun consentIntent(): Intent? = vpn.consentIntent()

    /** Starts the VPN again, with consent given; the state follows from the VPN's own. */
    fun reconnect() = act { vpn.start() }

    fun consentRefused() = messages.show(UiMessage(R.string.feature_overview_vpn_refused))

    fun batteryExemptionIntent(): Intent? = vpn.requestBatteryExemptionIntent()

    /**
     * Downloads the lists now. Asked for by the user, it runs on any network,
     * as Filters' "Update now" does; the background update keeps to Wi-Fi if
     * the settings say so. How each list fared is read back with the overview.
     */
    fun downloadLists() {
        if (download?.isActive == true) return
        download = viewModelScope.launchCatching("overview download", onFailure = {
            messages.show(UiMessage(R.string.feature_overview_lists_download_failed))
        }) {
            lists.update().collect {}
        }
    }

    fun dismissNotice(notice: OverviewNotice) {
        viewModelScope.launch {
            suspendRunCatching {
                if (notice == OverviewNotice.BatteryExemption) vpn.dismissBatteryExemption() else userData.dismissTip(notice.key)
            }
            refresh()
        }
    }

    /**
     * Requests root filtering. The VPN keeps filtering while root mode and
     * its settings are prepared; Settings sets root mode up where it is not.
     */
    fun switchToRoot() = act {
        try {
            if (methods.switchTo(FilterMethod.Root) == FilterMethodSwitch.Complete) {
                suspendRunCatching { userData.dismissTip(ROOT_TIP) }
            } else {
                messages.show(UiMessage(R.string.feature_overview_root_pending))
            }
        } finally {
            refresh()
        }
    }

    private fun act(block: suspend () -> Unit) {
        viewModelScope.launchCatching("overview action", onFailure = { e ->
            messages.show(UiMessage(if (e is AbdException.TimedOut) R.string.feature_overview_service_timeout else R.string.feature_overview_action_failed))
        }) {
            block()
        }
    }

    private companion object {
        const val TOP_APPS = 3

        /** In All apps, the VPN routes the apps the system lists: where it withholds the list, most go unfiltered. */
        fun appsUnseen(data: UserData, status: DaemonStatus, inventory: AppInventory<*>): Boolean = data.capabilities.usesVpn && status.filteringScope == FilteringScope.All && inventory.listWithheld

        /**
         * Root setup can still hold the list choices; an update is refused
         * until they are saved.
         */
        fun listDownloads(subscriptions: List<FilterList>, data: UserData): ListDownloads? {
            if (data.method == FilterMethod.Root && data.hasPendingLists) return null
            val missing = subscriptions.filter { it.awaitsFirstDownload }
            val previous = subscriptions.count { it.enabled && it.usingPreviousAddress && it.error != null }
            if (missing.isEmpty() && previous == 0) return null
            return ListDownloads(missing.size, missing.any { it.error != null }, previous)
        }

        const val LOCKDOWN_TIP = "vpn:lockdown"
        const val ROOT_TIP = "vpn:root-available"

        val OverviewNotice.key: String get() = when (this) {
            OverviewNotice.Lockdown -> LOCKDOWN_TIP
            OverviewNotice.RootAvailable -> ROOT_TIP
            OverviewNotice.BatteryExemption -> error("Battery notice follows the observed VPN stop")
            OverviewNotice.ListDownloads, OverviewNotice.AppListWithheld -> error("$this is a state, not a tip")
        }
    }
}
