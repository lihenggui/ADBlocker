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

package li.mercury.adblocker.core.ui.coil

import android.content.Context
import android.content.pm.LauncherApps
import android.content.pm.PackageManager
import android.graphics.drawable.Drawable
import android.os.UserManager
import androidx.annotation.Px
import coil3.ImageLoader
import coil3.asImage
import coil3.decode.DataSource
import coil3.fetch.FetchResult
import coil3.fetch.Fetcher
import coil3.fetch.ImageFetchResult
import coil3.request.Options
import li.mercury.adblocker.core.model.AppIconSource
import me.zhanghai.android.appiconloader.AppIconLoader

/*
 * Includes code from Blocker, modified for ADBlocker.
 * Copyright 2026 Blocker
 * Copyright 2020 Google LLC
 * Upstream license: Apache-2.0. See THIRD_PARTY_NOTICES.md.
 */

/**
 * Loads an app's icon for Coil, drawn the way a
 * launcher draws it. Coil runs this off the main thread and keeps the result
 * in memory, keyed by [AppIconKeyer].
 */
class AppIconFetcher private constructor(
    private val loader: AppIconLoader,
    private val context: Context,
    private val source: AppIconSource,
) : Fetcher {
    override suspend fun fetch(): FetchResult? {
        return try {
            val image = if (source.work == null) {
                loader.loadIcon(context.packageManager.getApplicationInfo(source.packageName, 0)).asImage()
            } else {
                workIcon()?.asImage() ?: return null
            }
            ImageFetchResult(image, isSampled = true, dataSource = DataSource.DISK)
        } catch (_: SecurityException) {
            // A work profile can disappear between listing and loading its icon.
            null
        } catch (_: PackageManager.NameNotFoundException) {
            null
        }
    }

    private fun workIcon(): Drawable? {
        val work = source.work ?: return null
        val users = context.getSystemService(UserManager::class.java) ?: return null
        if (users.getSerialNumberForUser(work.user) != work.serial) return null
        val launcher = context.getSystemService(LauncherApps::class.java) ?: return null
        if (work.activity != null) {
            val activity = launcher.getActivityList(source.packageName, work.user).firstOrNull { it.componentName == work.activity }
            if (activity != null) return activity.getBadgedIcon(0)
        }
        val pm = context.packageManager
        val app = launcher.getApplicationInfo(source.packageName, 0, work.user)
        return pm.getUserBadgedIcon(app.loadIcon(pm), work.user)
    }

    /**
     * @param iconSize the size icons are drawn at, in pixels.
     * @param shrinkNonAdaptiveIcons whether to fit legacy icons into the
     *   adaptive icon shape, as launchers do.
     */
    class Factory(
        @Px iconSize: Int,
        shrinkNonAdaptiveIcons: Boolean,
        private val context: Context,
    ) : Fetcher.Factory<AppIconSource> {
        private val loader = AppIconLoader(iconSize, shrinkNonAdaptiveIcons, context.applicationContext)

        override fun create(data: AppIconSource, options: Options, imageLoader: ImageLoader): Fetcher = AppIconFetcher(loader, context, data)
    }
}
