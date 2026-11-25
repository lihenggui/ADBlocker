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

package li.mercury.adblocker.core.data.repository

import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.AbdWriteLock
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.EngineTransport
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.SavedAppKey
import li.mercury.adblocker.core.model.VpnTransferPart
import timber.log.Timber
import javax.inject.Inject
import javax.inject.Singleton

@Singleton
internal class DefaultFilterMethodRepository(
    private val rootShell: RootShell,
    private val userData: UserDataRepository,
    /** The transport of each method, whichever is in use. */
    private val transportOf: (FilterMethod) -> AbdTransport,
    private val vpn: VpnController,
    private val module: ModuleRepository,
    private val changes: DataChanges,
    private val updates: ListUpdateScheduler,
    private val writes: AbdWriteLock,
) : FilterMethodRepository {
    @Inject
    constructor(
        rootShell: RootShell,
        userData: UserDataRepository,
        root: RootTransport,
        engine: EngineTransport,
        vpn: VpnController,
        module: ModuleRepository,
        changes: DataChanges,
        updates: ListUpdateScheduler,
        writes: AbdWriteLock,
    ) : this(
        rootShell,
        userData,
        { method ->
            when (method) {
                FilterMethod.Root -> root
                FilterMethod.Vpn -> engine
            }
        },
        vpn,
        module,
        changes,
        updates,
        writes,
    )

    private val switching = Mutex()

    override fun rootGrantKnown(): Boolean = rootShell.rootGrantKnown()

    override suspend fun rootAvailable(): Boolean = try {
        rootShell.run("true", label = "root check").isSuccess
    } catch (_: AbdException.NoRoot) {
        false
    }

    override suspend fun switchTo(method: FilterMethod): FilterMethodSwitch = switching.withLock {
        var settings = userData.userData.first()
        if (settings.rootSwitchPending && (settings.rootSwitchCancelling || method == FilterMethod.Vpn)) {
            cancelRootSwitch(settings.method)
            if (settings.method == method) return FilterMethodSwitch.Complete
            settings = userData.userData.first()
        }
        if (method == FilterMethod.Root && (settings.method == FilterMethod.Vpn || settings.rootSwitchPending)) {
            // The engine already stores the source configuration privately.
            // Keep using it while the module installs, and read it again
            // on retry so choices made in the meantime are also carried.
            if (!settings.rootSwitchPending) userData.setRootSwitchPending()
            return if (completeRootSwitch()) FilterMethodSwitch.Complete else FilterMethodSwitch.RootPending
        }
        if (settings.method == method) {
            userData.setFilterMethod(method)
            return FilterMethodSwitch.Complete
        }
        // VPN setup always records its method. Root setup can finish with the method unset.
        val rootWasConfigured = settings.filterMethod == FilterMethod.Root || (settings.filterMethod == null && settings.onboardingDone)
        val transfer = writes.mutex.withLock { switchToVpn(rootWasConfigured) }
        if (transfer.listsAdded) scheduleUpdates()
        when {
            transfer.rootUnreadable -> FilterMethodSwitch.VpnTransferIncomplete()
            transfer.failed.isNotEmpty() -> FilterMethodSwitch.VpnTransferIncomplete(transfer.failed.toSet())
            else -> FilterMethodSwitch.Complete
        }
    }

    override suspend fun retryRootSwitch(): Boolean = switching.withLock {
        val settings = userData.userData.first()
        if (!settings.rootSwitchPending) {
            if (settings.rootHandoffCleanupPending) finishRootHandoff()
            return@withLock false
        }
        if (settings.rootSwitchCancelling) {
            cancelRootSwitch(settings.method)
            return@withLock false
        }
        completeRootSwitch()
    }

    override suspend fun cancelRootSwitch() = switching.withLock {
        val settings = userData.userData.first()
        if (settings.rootSwitchPending) cancelRootSwitch(settings.method)
    }

    private suspend fun cancelRootSwitch(activeMethod: FilterMethod) {
        // A later launch should resume the rollback if this attempt fails.
        userData.setRootSwitchCancelling()
        changes.changing {
            val root = transportOf(FilterMethod.Root)
            userData.rootAppsBeforeSwitch()?.let { restoreRootApps(root.fixedAbdClient(), it) }
            userData.rootHandoffAttempt()?.let { attempt ->
                handoff(root, "rollback", attempt)
                updates.updateRootSoon()
            }
            userData.setFilterMethod(activeMethod)
        }
    }

    private suspend fun completeRootSwitch(): Boolean {
        if (module.state() != ModuleState.Ready || !finishRootHandoff()) return false
        val from = transportOf(FilterMethod.Vpn).fixedAbdClient()
        val root = transportOf(FilterMethod.Root)
        val to = root.fixedAbdClient()
        // Let a hot installation start before asking the service script to
        // start it. That script also refuses a disabled or removed module.
        if (!awaitRunning(to) && !module.startDaemon()) return false

        // A failed read may look like an empty preference to a partial
        // copy. Read both configurations fully before changing anything.
        val wanted = configuration(from)
        val had = configuration(to)
        val switched = changes.changing {
            // A crash after begin can leave a root snapshot. Remember its
            // identity first, and use it through any retry of this switch.
            beginRootHandoff(root)
            writeRootConfiguration(to, wanted, had)
            writes.mutex.withLock {
                // Let writes already routed to the VPN finish, then verify
                // their results before publishing the new method. A queued
                // write picks its transport only after publication.
                if (configuration(from) != wanted || !to.status().running) return@withLock false
                activateRoot(to, wanted.apps)
            }
        }
        if (!switched) return false
        suspendRunCatching { vpn.stop() }.onFailure { Timber.w("method switch: vpn stop: %s", it.javaClass.simpleName) }
        finishRootHandoff()
        return true
    }

    private suspend fun beginRootHandoff(root: AbdTransport) {
        val existing = userData.rootHandoffAttempt()
        val attempt = existing ?: userData.startRootHandoff()
        try {
            handoff(root, "begin", attempt)
        } catch (e: AbdException.CommandRejected) {
            // An unknown command cannot have created this first snapshot.
            // A retry might already have a snapshot from an earlier module.
            if (existing == null) userData.clearRootHandoffAttempt()
            throw e
        }
    }

    private suspend fun handoff(root: AbdTransport, action: String, token: String) {
        root.run(listOf("handoff", action, token), label = "handoff $action")
    }

    private suspend fun finishRootHandoff(): Boolean = suspendRunCatching {
        userData.rootHandoffToFinish()?.let { token ->
            handoff(transportOf(FilterMethod.Root), "finish", token)
            updates.updateRootSoon()
            userData.clearRootHandoffToFinish()
        }
    }.onFailure { Timber.w("method switch: finish: %s", it.javaClass.simpleName) }.isSuccess

    private data class HttpsPreferences(val urlFiltering: Boolean?, val cosmetic: Boolean?)

    private fun DaemonStatus?.httpsPreferences() = HttpsPreferences(this?.urlFiltering, this?.cosmetic)

    private suspend fun carryHttps(
        to: AbdClient,
        wanted: HttpsPreferences,
        current: HttpsPreferences,
        onMissing: () -> Unit = {},
        apply: suspend (suspend () -> Unit) -> Unit,
    ) {
        suspend fun carry(value: Boolean?, previous: Boolean?, write: suspend (Boolean) -> Unit) {
            if (value == null) {
                onMissing()
            } else if (value != previous) {
                apply { write(value) }
            }
        }

        carry(wanted.urlFiltering, current.urlFiltering, to::setUrlFiltering)
        carry(wanted.cosmetic, current.cosmetic, to::setCosmetic)
    }

    private data class Configuration(
        val apps: AppSelections,
        val lists: Map<String, Boolean>,
        val rules: String,
        val trusted: Set<String>,
        val https: HttpsPreferences,
    )

    private suspend fun configuration(client: AbdClient) = Configuration(
        apps = client.appSelections(),
        lists = client.lists().associate { it.url to it.enabled },
        rules = client.userRules(),
        trusted = client.allowList().toSet(),
        https = client.status().httpsPreferences(),
    )

    private suspend fun writeRootConfiguration(to: AbdClient, wanted: Configuration, had: Configuration) {
        // The new apps are joined only after their filtering settings are ready.
        carryHttps(to, wanted.https, had.https) { it() }
        for (host in wanted.trusted - had.trusted) to.allow(host)
        for (url in wanted.lists.keys - had.lists.keys) to.addList(url)
        for ((url, enabled) in wanted.lists) {
            if ((had.lists[url] ?: true) != enabled) to.setListEnabled(url, enabled)
        }
        if (wanted.rules != had.rules) to.setUserRules(wanted.rules)
        for (url in had.lists.keys - wanted.lists.keys) to.removeList(url)
        for (host in had.trusted - wanted.trusted) to.disallow(host)
    }

    // Only personal selections take part in a method handoff. The module
    // keeps its own work selection while the personal method changes.
    private suspend fun personalChoices(client: AbdClient) = client.appSelections().chosen.filterTo(mutableSetOf()) { SavedAppKey(it).isPersonal }

    private suspend fun restoreRootApps(to: AbdClient, wanted: Set<String>) {
        val current = personalChoices(to)
        for (app in current - wanted) to.removeApp(app)
        for (app in wanted - current) to.addApp(app)
        if (personalChoices(to) != wanted) throw AbdException.CommandFailed("The previous root app selection could not be restored")
    }

    private suspend fun activateRoot(to: AbdClient, wanted: AppSelections): Boolean {
        val personal = to.apps().filter { it.profileSerial == null }
        val current = personalChoices(to)
        // Root groups keep their saved selection. Carrying a VPN package into
        // a group would authorize traffic from apps outside that selection.
        val members = to.appEligibility().groups.flatMapTo(mutableSetOf()) { it.packages }
        val transferable = wanted.chosen.filterTo(mutableSetOf()) { it !in members && SavedAppKey(it).isTransferablePersonalPackage }
        val preserved = personal.filter { it.unavailableReason != null || AppIdentity(it.packageName).isSharedGroup }.map { it.packageName }.toSet()
        val previous = userData.rootAppsBeforeSwitch() ?: current.also { userData.rememberRootAppsBeforeSwitch(it) }
        // Keep the original selection past process death. Once capture can
        // change, a cancelled caller still finishes publishing or restoring.
        return withContext(NonCancellable) {
            var activated = false
            try {
                for (app in transferable - preserved - current) to.addApp(app)
                for (app in current - transferable - preserved) to.removeApp(app)
                if (wanted.scope == FilteringScope.Chosen) to.setFilteringScope(FilteringScope.Chosen)
                to.configureExclusions(wanted.excluded)
                if (wanted.scope == FilteringScope.All) to.setFilteringScope(FilteringScope.All)
                if (!to.status().running) return@withContext false
                userData.publishRootHandoff()
                activated = true
                true
            } finally {
                if (!activated) {
                    suspendRunCatching { restoreRootApps(to, previous) }.onFailure {
                        Timber.w("method switch: restore root apps: %s", it.javaClass.simpleName)
                    }
                }
            }
        }
    }

    private suspend fun switchToVpn(rootWasConfigured: Boolean): Written = changes.changing {
        val from = transportOf(FilterMethod.Root).fixedAbdClient()
        val carried = suspendRunCatching { read(from) }.getOrElse { e ->
            userData.setFilterMethod(FilterMethod.Vpn)
            if (rootWasConfigured) Timber.w("method switch: read apps: %s", e.javaClass.simpleName)
            return@changing Written(rootUnreadable = rootWasConfigured)
        }
        if (!rootWasConfigured && !carried.hasChoices) {
            // A fresh module already returns built-in defaults. Leave any VPN settings in place.
            userData.setFilterMethod(FilterMethod.Vpn)
            return@changing Written()
        }

        val to = transportOf(FilterMethod.Vpn).fixedAbdClient()
        val written = writeToVpn(to, carried)
        val previous = suspendRunCatching { to.appSelections() }.getOrNull()
        // The VPN uses personal packages. An identity in another profile
        // must never turn on its personal instance or replace saved work choices.
        val savedWork = previous?.chosen.orEmpty().filterNotTo(mutableSetOf()) { SavedAppKey(it).isPersonal }
        val wanted = carried.apps.copy(chosen = savedWork + carried.personal)
        val verified = previous != null && suspendRunCatching { from.appSelections() == carried.apps }.getOrDefault(false)
        if (!publishVpn(to, previous.takeIf { verified }, wanted)) written.failed += VpnTransferPart.Apps
        if (written.failed.isNotEmpty()) {
            Timber.w("method switch: VPN transfer incomplete")
        } else {
            Timber.i("method switch: %d items carried", written.done)
        }
        written
    }

    private suspend fun publishVpn(to: AbdClient, previous: AppSelections?, wanted: AppSelections): Boolean = withContext(NonCancellable) {
        if (previous == null) {
            userData.setFilterMethod(FilterMethod.Vpn)
            return@withContext false
        }
        // An app policy spans several commands. Publish only after it is
        // verified, or after the previous policy has been restored.
        val carried = previous == wanted || suspendRunCatching {
            writeVpnSelection(to, previous, wanted)
            if (to.appSelections() != wanted) throw AbdException.CommandFailed("VPN app policy could not be verified")
        }.isSuccess
        if (!carried) restoreVpnSelection(to, previous)
        try {
            userData.setFilterMethod(FilterMethod.Vpn)
        } catch (e: Exception) {
            if (carried && previous != wanted) restoreVpnSelection(to, previous)
            throw e
        }
        carried
    }

    private suspend fun restoreVpnSelection(to: AbdClient, previous: AppSelections) {
        // A reload can fail after a write, so check what was restored.
        suspendRunCatching { writeVpnSelection(to, to.appSelections(), previous) }
        if (!suspendRunCatching { to.appSelections() == previous }.getOrDefault(false)) {
            throw AbdException.CommandFailed("The previous VPN app policy could not be restored")
        }
    }

    private suspend fun writeVpnSelection(to: AbdClient, current: AppSelections, wanted: AppSelections) {
        // Remove obsolete selections before adding any. An All apps scope
        // is enabled only after its exclusions are in place.
        for (app in current.chosen - wanted.chosen) to.removeApp(app)
        for (app in wanted.chosen - current.chosen) to.addApp(app)
        if (wanted.scope != current.scope && wanted.scope == FilteringScope.Chosen) to.setFilteringScope(FilteringScope.Chosen)
        if (wanted.excluded != current.excluded) to.configureExclusions(wanted.excluded)
        if (wanted.scope != current.scope && wanted.scope == FilteringScope.All) to.setFilteringScope(FilteringScope.All)
    }

    private suspend fun scheduleUpdates() {
        suspendRunCatching { updates.updateSoon() }.onFailure { Timber.w("method switch: update: %s", it.javaClass.simpleName) }
    }

    private class Carried(
        val apps: AppSelections,
        val lists: List<FilterList>?,
        val rules: String?,
        val trusted: List<String>?,
        val https: HttpsPreferences,
    ) {
        val personal = apps.chosen.filter { SavedAppKey(it).isTransferablePersonalPackage }
        val hasChoices: Boolean
            get() = personal.isNotEmpty() || apps.excluded.isNotEmpty() || apps.scope == FilteringScope.All ||
                !rules.isNullOrEmpty() || !trusted.isNullOrEmpty() || https.urlFiltering == false || https.cosmetic == false ||
                lists.orEmpty().any { !it.enabled || !FilterCatalog.canBeDefault(it.url) }
    }

    private class Written(val rootUnreadable: Boolean = false) {
        val failed = mutableSetOf<VpnTransferPart>()
        var done = 0
        var listsAdded = false
    }

    /** What [from] has, each part on its own; throws if its apps cannot be read at all. */
    private suspend fun read(from: AbdClient): Carried {
        val apps = from.appSelections()
        val status = suspendRunCatching { from.status() }.getOrNull()
        return Carried(
            apps = apps,
            lists = suspendRunCatching { from.lists() }.getOrNull(),
            rules = suspendRunCatching { from.userRules() }.getOrNull(),
            trusted = suspendRunCatching { from.allowList() }.getOrNull(),
            https = status.httpsPreferences(),
        )
    }

    private suspend fun writeToVpn(to: AbdClient, carried: Carried): Written {
        val written = Written()

        suspend fun <T> read(part: VpnTransferPart, read: suspend () -> T): T? = suspendRunCatching { read() }.getOrElse {
            written.failed += part
            null
        }

        suspend fun each(part: VpnTransferPart, change: suspend () -> Unit): Boolean {
            val ok = suspendRunCatching { change() }.isSuccess
            if (ok) written.done++ else written.failed += part
            return ok
        }

        val current = suspendRunCatching { to.status() }.getOrNull()
        val urlPart = VpnTransferPart.UrlFiltering
        carryHttps(to, carried.https, current.httpsPreferences(), onMissing = { written.failed += urlPart }) { write ->
            each(urlPart, write)
        }

        val trustedPart = VpnTransferPart.TrustedSites
        val trusted = carried.trusted
        if (trusted == null) {
            written.failed += trustedPart
        } else {
            read(trustedPart) { to.allowList() }?.let { had ->
                had.filterNot { it in trusted }.forEach { each(trustedPart) { to.disallow(it) } }
                trusted.filterNot { it in had }.forEach { each(trustedPart) { to.allow(it) } }
            }
        }

        val rulesPart = VpnTransferPart.Rules
        val rules = carried.rules
        if (rules == null) {
            written.failed += rulesPart
        } else {
            read(rulesPart) { to.userRules() }?.let { had ->
                if (had != rules) each(rulesPart) { to.setUserRules(rules) }
            }
        }

        val listsPart = VpnTransferPart.Lists
        val lists = carried.lists
        if (lists == null) {
            written.failed += listsPart
        } else {
            read(listsPart) { to.lists().associate { it.url to it.enabled } }?.let { had ->
                val wanted = lists.map { it.url }.toSet()
                had.keys.filterNot { it in wanted }.forEach { each(listsPart) { to.removeList(it) } }
                for (list in lists) {
                    val enabled = had[list.url]
                    when {
                        enabled == null -> if (each(listsPart) { to.addList(list.url) }) {
                            written.listsAdded = true
                            if (!list.enabled) each(listsPart) { to.setListEnabled(list.url, false) }
                        }

                        enabled != list.enabled -> each(listsPart) { to.setListEnabled(list.url, list.enabled) }
                    }
                }
            }
        }
        return written
    }
}
