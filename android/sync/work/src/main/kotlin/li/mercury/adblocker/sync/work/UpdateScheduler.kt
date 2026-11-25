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
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.await
import androidx.work.workDataOf
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.UpdateInterval
import java.time.Duration
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.time.Duration.Companion.seconds

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
                    schedule(if (onboarded) settings.first else UpdateInterval.Never, settings.second)
                }
        }
        scope.launch { downloadMissing() }
    }

    private val missingLists = MissingListsCheck(
        versionCode = { context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode },
        userData = userData,
        readLists = lists::lists,
        requestUpdate = ::updateSoon,
        readTimeout = MISSING_LISTS_READ_TIMEOUT,
    )

    /** Asks for an update if a list that is on was never downloaded, once for each installed version. */
    suspend fun downloadMissing() = missingLists.run()

    private suspend fun schedule(interval: UpdateInterval, wifiOnly: Boolean) {
        val manager = workManager
        val existing = manager.getWorkInfosForUniqueWorkFlow(PERIODIC).first().filterNot { it.state.isFinished }
        if (interval == UpdateInterval.Never) {
            if (existing.isNotEmpty()) manager.cancelUniqueWork(PERIODIC).await()
            return
        }
        val period = Duration.ofDays(interval.days.toLong())
        val constraints = constraints(wifiOnly, batteryNotLow = true)
        if (existing.singleOrNull()?.matchesPeriodicUpdate(period, constraints) == true) return
        // The first run a whole period from now: onboarding has just
        // downloaded the lists, or asked for a download of its own. Enqueued
        // again with UPDATE, the work keeps the time it was first enqueued,
        // so starting the app does not put the update off.
        val request = PeriodicWorkRequestBuilder<UpdateListsWorker>(period)
            .setInitialDelay(period)
            .setConstraints(constraints)
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, UPDATE_BACKOFF)
            .build()
        manager.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, request).await()
    }

    override suspend fun updateSoon() {
        val wifiOnly = userData.userData.first().updateOnWifiOnly
        val request = oneTimeUpdate(wifiOnly)
        workManager.enqueueUniqueWork(ONCE, ExistingWorkPolicy.KEEP, request)
    }

    override suspend fun updateRootSoon() {
        val wifiOnly = userData.userData.first().updateOnWifiOnly
        val request = oneTimeUpdate(wifiOnly, rootOnly = true)
        // A previous update might already have read the staged sources. Enqueue
        // another so the published or restored sources are read again.
        workManager.enqueueUniqueWork(ROOT_ONCE, ExistingWorkPolicy.APPEND_OR_REPLACE, request).await()
    }

    companion object {
        /** How long [downloadMissing] waits for the lists: well under the limit of a broadcast receiver. */
        val MISSING_LISTS_READ_TIMEOUT = 8.seconds

        private const val PERIODIC = "update-lists"
        private const val ONCE = "update-lists-once"
        private const val ROOT_ONCE = "update-root-lists-once"
    }
}

private const val BACKOFF_MINUTES = 30L
private val UPDATE_BACKOFF = Duration.ofMinutes(BACKOFF_MINUTES)

internal fun WorkInfo.matchesPeriodicUpdate(period: Duration, constraints: Constraints): Boolean = !state.isFinished && workerClassName == UpdateListsWorker::class.java.name &&
    periodicityInfo == WorkInfo.PeriodicityInfo(period.toMillis(), period.toMillis()) &&
    initialDelayMillis == period.toMillis() && this.constraints == constraints

internal fun oneTimeUpdate(wifiOnly: Boolean, rootOnly: Boolean = false) = OneTimeWorkRequestBuilder<UpdateListsWorker>()
    .setConstraints(constraints(wifiOnly, batteryNotLow = false))
    .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, UPDATE_BACKOFF)
    .apply { if (rootOnly) setInputData(workDataOf(ROOT_UPDATE to true)) }
    .build()

private fun constraints(wifiOnly: Boolean, batteryNotLow: Boolean) = Constraints.Builder()
    .setRequiredNetworkType(if (wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED)
    .setRequiresBatteryNotLow(batteryNotLow)
    .build()
