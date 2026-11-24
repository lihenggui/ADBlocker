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

package li.mercury.adblocker.sync.work

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.await
import androidx.work.workDataOf
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.UpdateInterval
import java.time.Duration
import javax.inject.Inject
import javax.inject.Singleton

/** Keeps the background list update scheduled as the settings say. */
@Singleton
class UpdateScheduler @Inject constructor(
    @ApplicationContext private val context: Context,
    private val userData: UserDataRepository,
    private val lists: ListsRepository,
) : ListUpdateScheduler {
    private val workManager get() = WorkManager.getInstance(context)

    /** Follows the settings for as long as [scope] lives: the application's. */
    fun follow(scope: CoroutineScope) {
        scope.launch {
            userData.userData
                .map { it.onboardingDone to (it.updateInterval to it.updateOnWifiOnly) }
                .distinctUntilChanged()
                .collect { (onboarded, settings) ->
                    // Nothing to update before the module is set up.
                    if (onboarded) schedule(settings.first, settings.second) else workManager.cancelUniqueWork(PERIODIC)
                }
        }
        scope.launch { downloadMissing() }
    }

    /**
     * Asks for an update when a list that is on has never been downloaded.
     * A list that an update of the app made a default is on from then, and
     * the periodic update may be days away.
     */
    suspend fun downloadMissing() {
        if (!userData.userData.first().onboardingDone) return
        // Unreadable lists are read again when the app next starts.
        val missing = suspendRunCatching("lists before an update") { lists.lists() }.getOrNull()
            ?.let(::anyNeverDownloaded) ?: return
        if (missing) updateSoon()
    }

    private fun schedule(interval: UpdateInterval, wifiOnly: Boolean) {
        if (interval == UpdateInterval.Never) {
            workManager.cancelUniqueWork(PERIODIC)
            return
        }
        val period = Duration.ofDays(interval.days.toLong())
        // The first run a whole period from now: onboarding has just
        // downloaded the lists, or asked for a download of its own. Enqueued
        // again with UPDATE, the work keeps the time it was first enqueued,
        // so starting the app does not put the update off.
        val request = PeriodicWorkRequestBuilder<UpdateListsWorker>(period)
            .setInitialDelay(period)
            .setConstraints(constraints(wifiOnly, batteryNotLow = true))
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, Duration.ofMinutes(BACKOFF_MINUTES))
            .build()
        workManager.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, request)
    }

    override suspend fun updateSoon() {
        val wifiOnly = userData.userData.first().updateOnWifiOnly
        val request = OneTimeWorkRequestBuilder<UpdateListsWorker>()
            .setConstraints(constraints(wifiOnly, batteryNotLow = false))
            .build()
        workManager.enqueueUniqueWork(ONCE, ExistingWorkPolicy.KEEP, request)
    }

    override suspend fun updateRootSoon() {
        val wifiOnly = userData.userData.first().updateOnWifiOnly
        val request = OneTimeWorkRequestBuilder<UpdateListsWorker>()
            .setConstraints(constraints(wifiOnly, batteryNotLow = false))
            .setInputData(workDataOf(ROOT_UPDATE to true))
            .build()
        // A previous update might already have read the staged sources. Enqueue
        // another so the published or restored sources are read again.
        workManager.enqueueUniqueWork(ROOT_ONCE, ExistingWorkPolicy.APPEND_OR_REPLACE, request).await()
    }

    private fun constraints(wifiOnly: Boolean, batteryNotLow: Boolean) = Constraints.Builder()
        .setRequiredNetworkType(if (wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED)
        .setRequiresBatteryNotLow(batteryNotLow)
        .build()

    private companion object {
        const val PERIODIC = "update-lists"
        const val ONCE = "update-lists-once"
        const val ROOT_ONCE = "update-root-lists-once"
        const val BACKOFF_MINUTES = 30L
    }
}

/** Whether a list that is on has no copy yet. */
internal fun anyNeverDownloaded(lists: List<FilterList>): Boolean = lists.any { it.awaitsFirstDownload }
