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
import androidx.core.app.NotificationManagerCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import javax.inject.Inject

/** Android metadata and capabilities shown in Settings. */
interface AppPlatformInfo {
    fun versionName(): String

    fun notificationsAllowed(): Boolean

    /** Personal app labels, falling back to the package name if Android cannot resolve one. */
    suspend fun personalAppLabels(packages: Set<String>): Map<String, String>
}

internal class AndroidAppPlatformInfo @Inject constructor(
    @ApplicationContext private val context: Context,
    @Dispatcher(IO) private val io: CoroutineDispatcher,
) : AppPlatformInfo {
    override fun versionName(): String = context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty()

    override fun notificationsAllowed(): Boolean = NotificationManagerCompat.from(context).areNotificationsEnabled()

    override suspend fun personalAppLabels(packages: Set<String>): Map<String, String> = withContext(io) {
        val pm = context.packageManager
        packages.associateWith { name ->
            runCatching { pm.getApplicationLabel(pm.getApplicationInfo(name, 0)).toString() }.getOrDefault(name)
        }
    }
}
