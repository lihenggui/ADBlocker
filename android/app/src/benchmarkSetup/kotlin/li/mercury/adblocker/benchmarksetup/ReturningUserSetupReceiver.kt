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

package li.mercury.adblocker.benchmarksetup

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterMethod
import javax.inject.Inject

@AndroidEntryPoint
class ReturningUserSetupReceiver : BroadcastReceiver() {
    @Inject lateinit var settings: UserDataRepository

    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try {
                settings.setFilterMethod(FilterMethod.Vpn)
                settings.setShowSystemApps(true)
                settings.setOnboardingDone(true)
                pending.resultCode = Activity.RESULT_OK
            } finally {
                pending.finish()
            }
        }
    }
}
