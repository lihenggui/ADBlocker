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

package li.mercury.adblocker.core.model

import android.content.ComponentName
import android.os.UserHandle
import androidx.compose.runtime.Immutable

/**
 * A package or named shared UID in the personal user or in a work profile.
 * The profile serial identifies a profile even when Android reuses its user ID.
 */
data class AppIdentity(val packageName: String, val profileSerial: Long? = null) {
    init {
        require(packageName.isNotEmpty() && packageName != "@" && ':' !in packageName)
        require(profileSerial == null || profileSerial > 0)
    }

    val key: String get() = profileSerial?.let { "$it:$packageName" } ?: packageName
    val isSharedGroup: Boolean get() = packageName.hasSharedGroupPrefix

    /**
     * The package that a rule's `$app=` names for this app, in every profile;
     * null for a shared UID group, whose members a block cannot tell apart.
     */
    val rulePackage: String?
        get() = packageName.takeIf { name -> name.length <= MAX_PACKAGE_LENGTH && name.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '.' || it == '_' } }

    companion object {
        fun fromKey(key: String): AppIdentity = SavedAppKey(key).toIdentity()
    }
}

/** The original key is kept so handoffs can compare and restore saved selections verbatim. */
@JvmInline
value class SavedAppKey(private val key: String) {
    private val profileSeparator: Int get() = key.indexOf(':')

    /** Includes personal groups. Profile-shaped keys stay with their saved policy even if their serial is unreadable. */
    val isPersonal: Boolean get() = profileSeparator < 0

    /** The receiving backend validates each candidate when applying the transfer. */
    val isTransferablePersonalPackage: Boolean get() = isPersonal && !key.hasSharedGroupPrefix

    internal fun toIdentity(): AppIdentity {
        val separator = profileSeparator
        if (separator < 0) return AppIdentity(key)
        val prefix = key.substring(0, separator)
        val serial = prefix.toLongOrNull()
        require(serial != null && serial.toString() == prefix)
        return AppIdentity(key.substring(separator + 1), serial)
    }
}

private val String.hasSharedGroupPrefix: Boolean get() = startsWith('@')

/** The longest package name the engine reads in a rule. */
private const val MAX_PACKAGE_LENGTH = 255

/** An installed app, as the package manager describes it. */
@Immutable
data class InstalledApp(
    val packageName: String,
    val label: String,
    /** Where its icon is loaded from. */
    val icon: AppIconSource?,
    /** Part of the system and not in the launcher: hidden unless system apps are shown. */
    val isSystem: Boolean,
    val work: WorkApp? = null,
    val unavailableReason: AppUnavailableReason? = null,
    val sharedProfileSerial: Long? = null,
    val sharedMembers: List<SharedAppMember> = emptyList(),
    val uid: Int? = null,
) {
    val profileSerial: Long? get() = work?.serial ?: sharedProfileSerial
    val appKey: String get() = AppIdentity(packageName, profileSerial).key
    val isSharedGroup: Boolean get() = packageName.hasSharedGroupPrefix
    val canEnableFiltering: Boolean get() = unavailableReason == null && work?.available != false
}

/** An icon lookup and its version, without retaining a PackageInfo or launcher activity. */
data class AppIconSource(
    val packageName: String,
    val version: String,
    val work: WorkApp? = null,
)

/** A member of a named Android shared UID, with its installed label when visible. */
data class SharedAppMember(val packageName: String, val label: String)

/** App information the platform allows the personal user to read from a managed profile. */
data class WorkApp(
    val serial: Long,
    val user: UserHandle,
    val activity: ComponentName?,
    val quiet: Boolean,
    val available: Boolean,
)

/** Why the root module cannot filter an app separately. */
enum class AppUnavailableReason {
    SharedUid,
    Unavailable,
}

/**
 * The runtime permission that some systems, such as MIUI, HyperOS and others
 * following the same Chinese industry standard, require before they list the
 * installed apps.
 */
const val APP_LIST_PERMISSION = "com.android.permission.GET_INSTALLED_APPS"

/** Apps that could be read and any incomplete parts of their inventory. */
data class AppInventory<T>(
    val apps: List<T>,
    val workUnavailable: Boolean = false,
    val personalUnavailable: Boolean = false,
    /** The system withholds the installed app list from ADBlocker until it is allowed to read it. */
    val listWithheld: Boolean = false,
)

/** An installed app and whether, and how, it is filtered. */
@Immutable
data class AppEntry(
    val app: InstalledApp,
    /** Null when this app is outside the current filtering selection. */
    val filter: FilteredApp?,
    val inAllAppsScope: Boolean = false,
    val excluded: Boolean = false,
) {
    val packageName: String get() = app.packageName
    val appKey: String get() = app.appKey
    val unavailableReason: AppUnavailableReason? get() = filter?.unavailableReason ?: app.unavailableReason
    val isSelected: Boolean get() = filter != null
    val isFiltered: Boolean get() = isSelected && unavailableReason == null
    val canEnableFiltering: Boolean get() = unavailableReason == null && app.canEnableFiltering
}

/** How an app selection applies to choices in managed profiles. */
enum class WorkSelection { Replace, Keep }
