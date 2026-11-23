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

package li.mercury.adblocker.core.data.apps

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.LauncherActivityInfo
import android.content.pm.LauncherApps
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.content.pm.PackageManager.PackageInfoFlags
import android.content.pm.PackageManager.ResolveInfoFlags
import android.content.pm.ResolveInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.os.UserHandle
import android.os.UserManager
import androidx.core.content.ContextCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.runningFold
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.common.PERSONAL_APP_UIDS
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.engine.personalAppIdentities
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.WorkApp
import javax.inject.Inject
import javax.inject.Singleton

/** Apps that Android lets the current user see. */
@Singleton
class InstalledAppsSource @Inject constructor(
    @ApplicationContext private val context: Context,
    @Dispatcher(IO) private val ioDispatcher: CoroutineDispatcher,
    @ApplicationScope scope: CoroutineScope,
) {
    private val pm: PackageManager get() = context.packageManager
    private val launcherApps: LauncherApps? get() = context.getSystemService(LauncherApps::class.java)
    private val users: UserManager? get() = context.getSystemService(UserManager::class.java)

    @Volatile private var watchingPackages = false

    private val generations = callbackFlow {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                trySend(Unit)
            }
        }
        val profileEvents = IntentFilter().apply {
            addAction(Intent.ACTION_LOCALE_CHANGED)
            addAction(Intent.ACTION_MANAGED_PROFILE_ADDED)
            addAction(Intent.ACTION_MANAGED_PROFILE_REMOVED)
            addAction(Intent.ACTION_MANAGED_PROFILE_AVAILABLE)
            addAction(Intent.ACTION_MANAGED_PROFILE_UNAVAILABLE)
            addAction(Intent.ACTION_MANAGED_PROFILE_UNLOCKED)
        }
        val launcher = launcherApps
        val callback = object : LauncherApps.Callback() {
            override fun onPackageAdded(packageName: String, user: UserHandle) = changed()
            override fun onPackageRemoved(packageName: String, user: UserHandle) = changed()
            override fun onPackageChanged(packageName: String, user: UserHandle) = changed()
            override fun onPackagesAvailable(packageNames: Array<String>, user: UserHandle, replacing: Boolean) = changed()
            override fun onPackagesUnavailable(packageNames: Array<String>, user: UserHandle, replacing: Boolean) = changed()
            override fun onPackagesSuspended(packageNames: Array<String>, user: UserHandle) = changed()
            override fun onPackagesUnsuspended(packageNames: Array<String>, user: UserHandle) = changed()

            private fun changed() {
                trySend(Unit)
            }
        }
        ContextCompat.registerReceiver(context, receiver, profileEvents, ContextCompat.RECEIVER_EXPORTED)
        val registered = try {
            launcher?.registerCallback(callback, Handler(Looper.getMainLooper()))
            launcher != null
        } catch (_: SecurityException) {
            false
        }
        // The inventory is reusable only once package changes are observed.
        watchingPackages = registered
        trySend(Unit)
        awaitClose {
            watchingPackages = false
            if (registered) launcher?.unregisterCallback(callback)
            context.unregisterReceiver(receiver)
        }
    }.conflate().runningFold(0L) { generation, _: Unit -> generation + 1 }
        .stateIn(scope, SharingStarted.Lazily, 0L)

    val changes: Flow<Unit> = generations.drop(1).map { }

    private data class Personal(
        val app: InstalledApp,
        val internet: Boolean,
        val visibleSharedUid: Boolean,
    )

    private val inventory = InventoryCache<List<Personal>>()

    /** Every other installed app that can make connections of its own. */
    suspend fun apps(checkVisibleSharedUids: Boolean = false): List<InstalledApp> = personalInventory()
        .mapNotNull { (app, internet, shared) ->
            when {
                !internet -> null
                checkVisibleSharedUids && shared -> app.copy(unavailableReason = AppUnavailableReason.SharedUid)
                else -> app
            }
        }

    /** The personal rows for All apps, including apps with no internet permission. */
    suspend fun personalScopeApps(): List<InstalledApp> = personalInventory().mapNotNull { (app) ->
        app.takeIf { it.uid?.let(PERSONAL_APP_UIDS::contains) == true }
    }

    private suspend fun personalInventory(): List<Personal> = withContext(ioDispatcher) {
        val observing = withTimeoutOrNull(1_000) { generations.first { it > 0 } }
        if (observing == null || !watchingPackages) loadPersonal() else inventory.read({ generations.value }, ::loadPersonal)
    }

    private fun loadPersonal(): List<Personal> {
        val launchable = launcherPackages()
        val packages = pm.installedPackages(0)
        // Before installation Android can establish sharing for visible packages
        // only. A single visible owner does not establish root eligibility.
        val sharedUids = packages.mapNotNull { it.applicationInfo?.uid }.groupingBy { it }.eachCount().filterValues { it > 1 }.keys
        return packages
            .asSequence()
            .filter { it.packageName != context.packageName }
            .mapNotNull { info -> info.applicationInfo?.let { info to it } }
            .mapNotNull { (info, app) ->
                val internet = pm.checkPermission(INTERNET, info.packageName) == PackageManager.PERMISSION_GRANTED
                if (internet || app.uid in PERSONAL_APP_UIDS) {
                    Personal(personalApp(info, app, info.packageName in launchable), internet, app.uid in sharedUids)
                } else {
                    null
                }
            }
            .toList()
    }

    internal suspend fun personalIdentities() = withContext(ioDispatcher) { pm.personalAppIdentities(context.applicationInfo.uid) }

    suspend fun workApps(profiles: List<WorkProfile>, packageName: String? = null): AppInventory<InstalledApp> = withContext(ioDispatcher) {
        if (profiles.none { !it.packages.isNullOrEmpty() }) {
            return@withContext AppInventory(emptyList(), workUnavailable = profiles.any { it.packages == null })
        }
        suspendRunCatching {
            val launcher = checkNotNull(launcherApps)
            val users = checkNotNull(users)
            collectWorkApps(
                profiles,
                profileUsers = { launcher.profiles.filterNot { it == Process.myUserHandle() } },
                serialOf = users::getSerialNumberForUser,
                installed = { user, profile, packages ->
                    val activities = launcher.getActivityList(packageName, user)
                        .filterNot { it.componentName.className == APP_DETAILS_ACTIVITY }
                        .associateBy { it.componentName.packageName }
                    val quiet = users.isQuietModeEnabled(user)
                    packages.asSequence()
                        .filter { it != context.packageName && (packageName == null || it == packageName) }
                        .mapNotNull { name -> workApp(launcher, user, profile.serial, name, activities[name], quiet) }
                        .toList()
                },
            )
        }.getOrElse { AppInventory(emptyList(), workUnavailable = true) }
    }

    private fun workApp(
        launcher: LauncherApps,
        user: UserHandle,
        serial: Long,
        packageName: String,
        activity: LauncherActivityInfo?,
        quiet: Boolean,
    ): InstalledApp? {
        val app = try {
            activity?.applicationInfo ?: launcher.getApplicationInfo(packageName, 0, user)
        } catch (_: PackageManager.NameNotFoundException) {
            return null
        }
        val work = WorkApp(
            serial,
            user,
            activity?.componentName,
            quiet,
            available = !quiet && app.enabled && app.flags and ApplicationInfo.FLAG_SUSPENDED == 0,
        )
        return InstalledApp(
            packageName = packageName,
            label = activity?.label?.toString() ?: app.loadLabel(pm).toString(),
            icon = AppIconSource(packageName, app.sourceDir.orEmpty(), work),
            isSystem = isSystem(app, activity != null),
            work = work,
        )
    }

    /** One app, or null if it is not installed. */
    suspend fun app(packageName: String): InstalledApp? = withContext(ioDispatcher) {
        val info = try {
            pm.packageInfo(packageName, 0)
        } catch (_: PackageManager.NameNotFoundException) {
            return@withContext null
        }
        val app = info.applicationInfo ?: return@withContext null
        personalApp(info, app, packageName in launcherPackages())
    }

    private fun personalApp(info: PackageInfo, app: ApplicationInfo, launchable: Boolean): InstalledApp = InstalledApp(
        packageName = info.packageName,
        label = app.loadLabel(pm).toString(),
        icon = AppIconSource(info.packageName, info.lastUpdateTime.toString()),
        isSystem = isSystem(app, launchable),
        uid = app.uid,
    )

    /**
     * Part of the system and not opened by people: services, providers,
     * overlays. An app that came with the system but is in the launcher,
     * as Chrome and YouTube are on most phones, is an app like any other:
     * hidden, the browser people use most would never be offered.
     */
    private fun isSystem(app: ApplicationInfo, launchable: Boolean): Boolean = !launchable && app.flags and (ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0

    private fun launcherPackages(): Set<String> = pm.activitiesFor(Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER), PackageManager.MATCH_ALL)
        .mapNotNull { it.activityInfo?.packageName }
        .toSet()

    private companion object {
        const val INTERNET = "android.permission.INTERNET"
        const val APP_DETAILS_ACTIVITY = "android.app.AppDetailsActivity"
    }
}

/** Only the current package generation is held, shared by every caller. */
internal class InventoryCache<T : Any> {
    private val mutex = Mutex()
    private var cached: Pair<Long, T>? = null

    suspend fun read(generation: () -> Long, load: () -> T): T = mutex.withLock {
        val now = generation()
        cached?.takeIf { it.first == now }?.second ?: load().also { cached = now to it }
    }
}

/**
 * Visibility is checked separately for each profile. An empty daemon result
 * for a serial is authoritative, while an absent/failed Android observation
 * of a profile with packages leaves its apps unknown.
 */
internal suspend fun <User> collectWorkApps(
    profiles: List<WorkProfile>,
    profileUsers: () -> List<User>,
    serialOf: (User) -> Long,
    installed: (User, WorkProfile, Set<String>) -> List<InstalledApp>,
): AppInventory<InstalledApp> {
    val remaining = profiles.filter { !it.packages.isNullOrEmpty() }.associateBy(WorkProfile::serial).toMutableMap()
    var unavailable = profiles.any { it.packages == null }
    val visible = if (remaining.isEmpty()) {
        emptyList()
    } else {
        suspendRunCatching { profileUsers() }.getOrElse { return AppInventory(emptyList(), workUnavailable = true) }
    }
    val apps = buildList {
        for (user in visible) {
            val serial = suspendRunCatching { serialOf(user) }.getOrNull() ?: continue
            val profile = remaining.remove(serial) ?: continue
            val packages = checkNotNull(profile.packages)
            val result = suspendRunCatching { installed(user, profile, packages) }
            if (result.isSuccess) addAll(result.getOrThrow()) else unavailable = true
        }
    }
    return AppInventory(apps, workUnavailable = unavailable || remaining.isNotEmpty())
}

// The flag classes of these calls came with Android 13; the app runs from 12,
// where only the int overloads, deprecated since, exist.

private fun PackageManager.installedPackages(flags: Int): List<PackageInfo> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
    getInstalledPackages(PackageInfoFlags.of(flags.toLong()))
} else {
    @Suppress("DEPRECATION")
    getInstalledPackages(flags)
}

private fun PackageManager.packageInfo(packageName: String, flags: Int): PackageInfo = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
    getPackageInfo(packageName, PackageInfoFlags.of(flags.toLong()))
} else {
    @Suppress("DEPRECATION")
    getPackageInfo(packageName, flags)
}

private fun PackageManager.activitiesFor(intent: Intent, flags: Int): List<ResolveInfo> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
    queryIntentActivities(intent, ResolveInfoFlags.of(flags.toLong()))
} else {
    @Suppress("DEPRECATION")
    queryIntentActivities(intent, flags)
}
