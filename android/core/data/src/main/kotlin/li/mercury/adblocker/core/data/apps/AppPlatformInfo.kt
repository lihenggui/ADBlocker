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

import android.content.Context
import android.content.pm.PackageManager
import android.content.pm.PermissionInfo
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.model.APP_LIST_PERMISSION
import javax.inject.Inject

/** Android metadata and capabilities shown in Settings. */
interface AppPlatformInfo {
    /** Asks the package manager: off the main thread. */
    suspend fun versionName(): String

    /** Asks the notification manager, a binder call: off the main thread. */
    suspend fun notificationsAllowed(): Boolean

    /** Personal app labels, falling back to the package name if Android cannot resolve one. */
    suspend fun personalAppLabels(packages: Set<String>): Map<String, String>

    /**
     * Whether this system shows the installed apps only to an app granted
     * [APP_LIST_PERMISSION], and has not granted it. False where the
     * permission is not defined.
     */
    suspend fun appListWithheld(): Boolean
}

internal class AndroidAppPlatformInfo @Inject constructor(
    @ApplicationContext private val context: Context,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
) : AppPlatformInfo {
    override suspend fun versionName(): String = withContext(io) { context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty() }

    override suspend fun notificationsAllowed(): Boolean = withContext(io) { NotificationManagerCompat.from(context).areNotificationsEnabled() }

    override suspend fun personalAppLabels(packages: Set<String>): Map<String, String> = withContext(io) {
        val pm = context.packageManager
        packages.associateWith { name ->
            runCatching { pm.getApplicationLabel(pm.getApplicationInfo(name, 0)).toString() }.getOrDefault(name)
        }
    }

    override suspend fun appListWithheld(): Boolean = withContext(io) {
        val permission = try {
            context.packageManager.getPermissionInfo(APP_LIST_PERMISSION, 0)
        } catch (_: PackageManager.NameNotFoundException) {
            // Not defined: the system lists apps for QUERY_ALL_PACKAGES alone.
            return@withContext false
        }
        (permission.protectionLevel and PermissionInfo.PROTECTION_MASK_BASE) == PermissionInfo.PROTECTION_DANGEROUS &&
            ContextCompat.checkSelfPermission(context, APP_LIST_PERMISSION) != PackageManager.PERMISSION_GRANTED
    }
}
