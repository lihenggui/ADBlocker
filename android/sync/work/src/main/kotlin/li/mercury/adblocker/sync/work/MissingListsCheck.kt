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

import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterList
import kotlin.time.Duration

/**
 * Asks for an update when a list that is on has never been downloaded, once
 * for each installed version of the app. A list that an update of the app
 * made a default is on from then, and the periodic update may be days away.
 * Reading the lists runs a root command or opens the engine, so it is not
 * done at every start of the process.
 */
internal class MissingListsCheck(
    private val versionCode: () -> Long,
    private val userData: UserDataRepository,
    private val readLists: suspend () -> List<FilterList>,
    private val requestUpdate: suspend () -> Unit,
    private val readTimeout: Duration,
) {
    // The application's start and the receiver for an update can ask at once.
    private val mutex = Mutex()

    suspend fun run() {
        val version = versionCode()
        if (userData.listsCheckedVersion() == version) return
        mutex.withLock {
            if (userData.listsCheckedVersion() == version) return
            if (!userData.userData.first().onboardingDone) return
            // A read that fails or takes too long is left to the next start.
            val lists = withTimeoutOrNull(readTimeout) {
                suspendRunCatching("lists before an update") { readLists() }.getOrNull()
            } ?: return
            if (anyNeverDownloaded(lists)) requestUpdate()
            userData.setListsCheckedVersion(version)
        }
    }
}

/** Whether a list that is on has no copy yet. */
internal fun anyNeverDownloaded(lists: List<FilterList>): Boolean = lists.any { it.awaitsFirstDownload }
