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

package li.mercury.adblocker

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.sync.work.UpdateScheduler
import javax.inject.Inject

/**
 * Started when a new version of the app has been installed, which may
 * have made a list a default: the list is downloaded without the app
 * being opened first.
 */
@AndroidEntryPoint
class AppUpdatedReceiver : BroadcastReceiver() {
    @Inject
    lateinit var updateScheduler: UpdateScheduler

    @Inject
    @ApplicationScope
    lateinit var applicationScope: CoroutineScope

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        // Reading the lists can outlast this call.
        val pending = goAsync()
        applicationScope.launch {
            try {
                updateScheduler.downloadMissing()
            } finally {
                pending.finish()
            }
        }
    }
}
