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
import androidx.hilt.work.HiltWorker
import androidx.work.CoroutineWorker
import androidx.work.ListenableWorker
import androidx.work.WorkerParameters
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.lastOrNull
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.PendingSetup
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.UpdateEvent
import timber.log.Timber

internal const val ROOT_UPDATE = "root_update"

/**
 * Downloads the filter lists in the background, as neither the daemon nor
 * the engine does on a timer: `abd update` runs as root with the root
 * module, and in this process with the VPN method.
 */
@HiltWorker
class UpdateListsWorker @AssistedInject constructor(
    @Assisted context: Context,
    @Assisted params: WorkerParameters,
    private val lists: ListsRepository,
    private val userData: UserDataRepository,
    private val pending: PendingSetup,
) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val rootOnly = inputData.getBoolean(ROOT_UPDATE, false)
        return updateLists(lists, rootOnly = rootOnly) {
            val settings = userData.userData.first()
            if ((rootOnly || settings.method == FilterMethod.Root) && settings.hasPendingLists) {
                pending.applyIfReady()
            }
        }
    }
}

/**
 * Runs one update and says how it went, for WorkManager. Retried, with
 * backoff, is only what another try can fix: lists that could not be
 * fetched, or a command that failed or ran too long. No root, no module, a
 * rejected command (an older module's among them), a build without the
 * engine or an output it cannot read stay so until the user acts: retrying
 * them would run `su` again and again for nothing.
 */
internal suspend fun updateLists(lists: ListsRepository, rootOnly: Boolean = false, prepare: suspend () -> Unit = {}): ListenableWorker.Result = try {
    prepare()
    val events = if (rootOnly) lists.updateRoot() else lists.update()
    val done = events.filterIsInstance<UpdateEvent.Finished>().lastOrNull()
    Timber.i("list update: %d updated, %d unchanged, %d failed", done?.updated ?: 0, done?.unchanged ?: 0, done?.failed ?: 0)
    // Some lists failing is for the next run to fix; all failing, a retry with backoff.
    if (done == null || !done.exclusionsReady || (done.failed > 0 && done.updated + done.unchanged == 0)) {
        ListenableWorker.Result.retry()
    } else {
        ListenableWorker.Result.success()
    }
} catch (e: CancellationException) {
    throw e
} catch (e: Exception) {
    Timber.w("list update: %s", e.javaClass.simpleName)
    when (e) {
        is AbdException.NoRoot, is AbdException.ModuleMissing, is AbdException.CommandRejected, is AbdException.EngineMissing, is AbdException.BadOutput -> ListenableWorker.Result.failure()
        else -> ListenableWorker.Result.retry()
    }
}
