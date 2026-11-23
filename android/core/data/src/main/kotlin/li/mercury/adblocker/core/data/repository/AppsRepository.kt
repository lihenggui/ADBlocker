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

import kotlinx.coroutines.flow.first
import li.mercury.adblocker.core.common.PERSONAL_APP_UIDS
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.apps.InstalledAppsSource
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.protocol.SharedUidGroup
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.engine.isPersonalAppExcluded
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.model.WorkSelection
import javax.inject.Inject

/**
 * The installed apps and which of them are filtered, and how. An app
 * argument is its [AppIdentity.key], which includes the serial for work.
 */
interface AppsRepository {
    /** Every installed app with its filter state; system apps as the settings say. */
    suspend fun apps(): List<AppEntry>

    /** Inventory and whether some work apps could not be read. */
    suspend fun inventory(): AppInventory<AppEntry> = AppInventory(apps())

    suspend fun app(packageName: String): AppEntry?

    /** Setup inventory for the chosen method, with work apps when a supported module is available. */
    suspend fun installed(method: FilterMethod): AppInventory<InstalledApp>

    /** Applies confirmed setup choices, preserving saved work apps when the inventory was incomplete. */
    suspend fun configure(packages: Set<String>, work: WorkSelection = WorkSelection.Replace)

    /** Reconciles choices made before the module could check their UIDs. */
    suspend fun configureDeferred(packages: Set<String>, work: WorkSelection)

    /** Starts or stops filtering this app. */
    suspend fun setFiltered(packageName: String, filtered: Boolean)

    suspend fun pause(packageName: String, minutes: Int)

    suspend fun resume(packageName: String)

    /** The hosts the apps would not accept our certificate for; only [packageName]'s if given. */
    suspend fun pinned(packageName: String? = null): List<PinnedHost>

    suspend fun forgetPinned(packageName: String, host: String?)
}

internal class DefaultAppsRepository @Inject constructor(
    private val abd: AbdClient,
    private val installed: InstalledAppsSource,
    private val userData: UserDataRepository,
    private val changes: DataChanges,
    private val root: RootTransport,
) : AppsRepository {
    override suspend fun apps(): List<AppEntry> = inventory().apps

    override suspend fun inventory(): AppInventory<AppEntry> = inventory(requested = null)

    private suspend fun inventory(requested: AppIdentity?): AppInventory<AppEntry> {
        val settings = userData.userData.first()
        val selections = abd.appSelections()
        val selected = abd.apps().associateBy { it.appKey }
        val rootClient = abd.takeIf { settings.capabilities.managesModule }
        val personal = when {
            requested?.profileSerial != null -> AppInventory(emptyList())
            selections.scope == FilteringScope.All -> personalScopeInventory(rootClient, selected, requested, selections.excluded)
            else -> chosenScopeInventory(rootClient, selected, requested, settings.showSystemApps)
        }
        val work = if (rootClient != null && (requested == null || requested.profileSerial != null)) {
            readWorkEntries(rootClient, selected, requested)
        } else {
            AppInventory(emptyList())
        }
        val workEntries = work.apps.chosenEntries(selected, requested, settings.showSystemApps)
        return AppInventory(personal.apps + workEntries, work.workUnavailable, personal.personalUnavailable)
    }

    private suspend fun chosenScopeInventory(
        root: AbdClient?,
        selected: Map<String, FilteredApp>,
        requested: AppIdentity?,
        showSystemApps: Boolean,
    ): AppInventory<AppEntry> {
        val available = readPersonalEntries(root, selected, requested, loadApps = { installed.apps() })
        return AppInventory(
            available.apps.chosenEntries(selected, requested, showSystemApps),
            personalUnavailable = available.personalUnavailable,
        )
    }

    private suspend fun personalScopeInventory(
        root: AbdClient?,
        selected: Map<String, FilteredApp>,
        requested: AppIdentity?,
        exclusions: Set<String>,
    ): AppInventory<AppEntry> {
        val available = readPersonalEntries(root, selected, requested, loadApps = installed::personalScopeApps) {
            installed.personalIdentities().filter { it.key.startsWith('@') }.map {
                SharedUidGroup(it.key.removePrefix("@"), it.members)
            }
        }
        val entries = available.apps.map { app ->
            val eligible = app.isSharedGroup || app.uid?.let { it in PERSONAL_APP_UIDS } == true
            val excluded = eligible && isPersonalAppExcluded(app.appKey, app.sharedMembers.map { it.packageName }, exclusions)
            val row = app.withSelection(selected[app.appKey])
            AppEntry(row.app, row.filter, inAllAppsScope = eligible, excluded = excluded)
        }
        return AppInventory(entries, personalUnavailable = available.personalUnavailable)
    }

    private suspend fun readPersonalEntries(
        root: AbdClient?,
        selected: Map<String, FilteredApp>,
        requested: AppIdentity?,
        loadApps: suspend () -> List<InstalledApp>,
        vpnGroups: suspend () -> List<SharedUidGroup> = { emptyList() },
    ): AppInventory<InstalledApp> {
        val eligibility = root?.let { suspendRunCatching { it.appEligibility() }.getOrNull() }
        val groups = (if (root == null) vpnGroups() else eligibility?.groups.orEmpty()).matching(requested)
        val personal = when {
            requested == null || groups.isNotEmpty() -> loadApps()
            requested.isSharedGroup -> emptyList()
            else -> listOfNotNull(installed.app(requested.packageName))
        }
        val checked = if (root == null) AppInventory(personal) else withPersonalEligibility(personal, eligibility?.apps)
        return checked.copy(apps = withSharedGroups(checked.apps, groups, selected, requested, profileSerial = null))
    }

    private fun List<InstalledApp>.chosenEntries(
        selected: Map<String, FilteredApp>,
        requested: AppIdentity?,
        showSystemApps: Boolean,
    ): List<AppEntry> = filter {
        // A selected system app can still be removed when system apps are hidden.
        requested != null || showSystemApps || !it.isSystem || it.appKey in selected
    }.map { it.withSelection(selected[it.appKey]) }

    override suspend fun app(packageName: String): AppEntry? {
        val identity = AppIdentity.fromKey(packageName)
        val entries = inventory(identity)
        return entries.apps.firstOrNull { it.appKey == packageName }
            ?: entries.apps.firstOrNull { entry -> entry.app.sharedMembers.any { it.packageName == identity.packageName } }
            ?: if (entries.workUnavailable) throw AbdException.CommandFailed("Work app inventory is unavailable") else null
    }

    private suspend fun readWorkEntries(root: AbdClient, selected: Map<String, FilteredApp>, requested: AppIdentity?): AppInventory<InstalledApp> = withWorkInventory(emptyList(), root) { profiles ->
        val relevant = profiles.includingSavedSharedApps(selected.values).filter { requested == null || it.serial == requested.profileSerial }
        val scopedProfiles = relevant.map { profile ->
            val groups = profile.groups.orEmpty().matching(requested)
            val names = groups.flatMapTo(mutableSetOf(), SharedUidGroup::packages).apply { requested?.let { add(it.packageName) } }
            profile.copy(packages = profile.packages?.takeIf { requested == null } ?: profile.packages?.intersect(names), groups = groups)
        }
        val work = installed.workApps(scopedProfiles)
        val scopes = (scopedProfiles.map(WorkProfile::serial) + selected.values.mapNotNull { it.profileSerial }).distinct()
            .filter { requested == null || requested.profileSerial == it }
        val apps = scopes.flatMap { serial ->
            val groups = scopedProfiles.firstOrNull { it.serial == serial }?.groups.orEmpty()
            withSharedGroups(work.apps.filter { it.profileSerial == serial }, groups, selected, requested, serial)
        }
        work.copy(apps = apps)
    }.let { report ->
        if (report.workUnavailable && report.apps.isEmpty()) {
            val scopes = selected.values.mapNotNull { it.profileSerial }.distinct().filter { requested == null || requested.profileSerial == it }
            report.copy(apps = scopes.flatMap { serial -> withSharedGroups(emptyList(), emptyList(), selected, requested, serial) })
        } else {
            report
        }
    }

    override suspend fun installed(method: FilterMethod): AppInventory<InstalledApp> {
        val settings = userData.userData.first()
        // Setup can choose root before it becomes the active transport.
        val rootClient = if (method == FilterMethod.Root) root.fixedAbdClient() else null
        val eligibility = rootClient?.let { suspendRunCatching { it.appEligibility() } }
        val canCheckLocally = when (eligibility?.exceptionOrNull()) {
            is AbdException.ModuleMissing, is AbdException.CommandRejected -> true
            else -> false
        }
        val personal = installed.apps(checkVisibleSharedUids = canCheckLocally)
            .filter { settings.showSystemApps || !it.isSystem }
        val available = if (rootClient == null || canCheckLocally) {
            AppInventory(personal)
        } else {
            withPersonalEligibility(personal, eligibility?.getOrNull()?.apps)
        }
        val grouped = withSharedGroups(available.apps, eligibility?.getOrNull()?.groups.orEmpty(), emptyMap(), requested = null, profileSerial = null)
        val work = if (rootClient == null) AppInventory(emptyList()) else readWorkEntries(rootClient, emptyMap(), requested = null)
        return AppInventory(
            apps = (grouped + work.apps).filter { settings.showSystemApps || !it.isSystem },
            workUnavailable = work.workUnavailable,
            personalUnavailable = available.personalUnavailable,
        )
    }

    private enum class SelectionTiming { Immediate, Deferred }

    override suspend fun configure(packages: Set<String>, work: WorkSelection) = configure(packages, work, SelectionTiming.Immediate)

    override suspend fun configureDeferred(packages: Set<String>, work: WorkSelection) = configure(packages, work, SelectionTiming.Deferred)

    private suspend fun configure(packages: Set<String>, work: WorkSelection, timing: SelectionTiming) = changes.changing {
        val root = userData.userData.first().capabilities.managesModule
        // A failed personal inventory must not turn a retry into an empty replacement.
        val eligibility = if (root) abd.appEligibility() else null
        val choices = if (eligibility != null && timing == SelectionTiming.Deferred) {
            val groupMembers = eligibility.groups.flatMapTo(mutableSetOf(), SharedUidGroup::packages)
            reconcileDeferredSelection(packages, eligibility.apps, groupMembers)
        } else {
            packages
        }
        val keepWork = root && (work == WorkSelection.Keep || withWorkInventory(emptyList(), abd) { installed.workApps(it) }.workUnavailable)
        abd.configureApps(choices, if (keepWork) WorkSelection.Keep else WorkSelection.Replace)
    }

    private suspend fun reconcileDeferredSelection(packages: Set<String>, eligibility: Map<String, AppUnavailableReason?>, groupMembers: Set<String>): Set<String> {
        val shared = packages.filter { eligibility[it] == AppUnavailableReason.SharedUid && it !in groupMembers }
        if (shared.isEmpty()) return packages
        // Known new shared picks can never be added. Leave saved choices and
        // unknown results to the daemon's atomic selection validation.
        val saved = abd.apps().mapTo(HashSet(), FilteredApp::appKey)
        return packages - shared.filterNot(saved::contains).toSet()
    }

    override suspend fun setFiltered(packageName: String, filtered: Boolean) = changes.changing {
        val identity = AppIdentity.fromKey(packageName)
        if (identity.profileSerial == null && abd.appSelections().scope == FilteringScope.All) {
            abd.setAppExcluded(packageName, !filtered)
        } else if (filtered) {
            if (identity.isSharedGroup && !userData.userData.first().capabilities.managesModule) {
                throw AbdException.CommandFailed("Shared app groups require root")
            }
            if (!identity.isSharedGroup && identity.profileSerial != null && app(packageName)?.canEnableFiltering != true) {
                throw AbdException.CommandFailed("App filtering is unavailable")
            }
            abd.addApp(packageName)
        } else {
            abd.removeApp(packageName)
        }
    }

    override suspend fun pause(packageName: String, minutes: Int) = changes.changing { abd.pauseApp(packageName, minutes) }

    override suspend fun resume(packageName: String) = changes.changing { abd.resumeApp(packageName) }

    override suspend fun pinned(packageName: String?): List<PinnedHost> = abd.pinned(packageName)

    override suspend fun forgetPinned(packageName: String, host: String?) = changes.changing { abd.forgetPinned(packageName, host) }
}

/** Reads work apps separately so a root or Android lookup failure leaves the personal inventory intact. */
internal suspend fun withWorkInventory(
    personal: List<InstalledApp>,
    root: AbdClient?,
    workApps: suspend (List<WorkProfile>) -> AppInventory<InstalledApp>,
): AppInventory<InstalledApp> {
    if (root == null) return AppInventory(personal)
    val work = suspendRunCatching { workApps(root.workProfiles()) }
        .getOrElse { return AppInventory(personal, workUnavailable = true) }
    return AppInventory(personal + work.apps, work.workUnavailable)
}

private fun InstalledApp.withSelection(filter: FilteredApp?): AppEntry {
    // Keep known unavailability with the row while an optimistic removal
    // temporarily clears its selection.
    val shown = filter?.unavailableReason?.let { copy(unavailableReason = it) } ?: this
    return AppEntry(shown, filter)
}

private fun List<SharedUidGroup>.matching(requested: AppIdentity?): List<SharedUidGroup> = if (requested == null) {
    this
} else {
    filter { it.target == requested.packageName || requested.packageName in it.packages }
}

/** A named group contributes one filtering row, even when multiple members are visible. */
internal fun withSharedGroups(
    apps: List<InstalledApp>,
    groups: List<SharedUidGroup>,
    selected: Map<String, FilteredApp>,
    requested: AppIdentity?,
    profileSerial: Long?,
): List<InstalledApp> {
    val installed = apps.associateBy(InstalledApp::packageName)
    val current = groups.associateBy(SharedUidGroup::target)
    val saved = selected.values.filter { it.profileSerial == profileSerial && it.packageName.startsWith('@') }
        .filter { requested == null || it.appKey == requested.key }
    val shared = (current.keys + saved.map(FilteredApp::packageName)).map { target ->
        val group = current[target]
        val members = group?.packages.orEmpty().sorted()
        val visible = members.mapNotNull(installed::get)
        val representative = visible.firstOrNull { !it.isSystem && it.work?.available != false }
            ?: visible.firstOrNull { it.work?.available != false }
            ?: visible.firstOrNull()
        val filter = selected[AppIdentity(target, profileSerial).key]
        InstalledApp(
            packageName = target,
            label = representative?.label ?: target.removePrefix("@"),
            icon = representative?.icon,
            isSystem = visible.all(InstalledApp::isSystem),
            work = representative?.work,
            unavailableReason = filter?.unavailableReason ?: AppUnavailableReason.Unavailable.takeIf { group == null },
            sharedProfileSerial = profileSerial,
            sharedMembers = members.map { SharedAppMember(it, installed[it]?.label ?: it) },
        )
    }
    val memberNames = shared.flatMapTo(mutableSetOf()) { entry -> entry.sharedMembers.map(SharedAppMember::packageName) }
    return apps.filter { app ->
        (requested == null || app.packageName == requested.packageName) &&
            (app.packageName !in memberNames || app.appKey in selected)
    } + shared
}

/** An omitted package has no eligibility result in the daemon's snapshot. */
internal fun withPersonalEligibility(
    apps: List<InstalledApp>,
    eligibility: Map<String, AppUnavailableReason?>?,
): AppInventory<InstalledApp> {
    val unavailable = eligibility == null || apps.any { it.packageName !in eligibility }
    val decorated = apps.map { app ->
        val reason = eligibility?.getOrDefault(app.packageName, AppUnavailableReason.Unavailable)
        app.copy(unavailableReason = if (eligibility == null) AppUnavailableReason.Unavailable else reason)
    }
    return AppInventory(decorated, personalUnavailable = unavailable)
}

/** A currently installed, saved shared-UID work app remains removable. */
private fun List<WorkProfile>.includingSavedSharedApps(selected: Collection<FilteredApp>): List<WorkProfile> {
    val shared = selected.filter { it.profileSerial != null && !it.packageName.startsWith('@') && it.unavailableReason == AppUnavailableReason.SharedUid }
        .groupBy({ it.profileSerial }, { it.packageName })
    if (shared.isEmpty()) return this
    return map { profile -> profile.copy(packages = profile.packages?.plus(shared[profile.serial].orEmpty())) }
}
