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

package li.mercury.adblocker.core.data.repository

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.UpdateEvent
import javax.inject.Inject
import javax.inject.Singleton
import kotlin.time.Duration.Companion.minutes

/** The longest a list update may take, as long as WorkManager gives a worker. */
internal val UPDATE_TIMEOUT = 10.minutes

/** The filter list subscriptions. */
interface ListsRepository {
    suspend fun lists(): List<FilterList>

    /** Whether a list update runs in the app, whoever started it: the background update, Filters, onboarding or the Overview. */
    val updating: StateFlow<Boolean>

    suspend fun setEnabled(url: String, enabled: Boolean)

    /** Saves the displayed choices together, leaving other subscriptions in place. */
    suspend fun configure(choices: Map<String, Boolean>)

    /** Subscribes to [url] or enables it again. It is downloaded at the next update. */
    suspend fun add(url: String)

    suspend fun remove(url: String)

    /**
     * Downloads every enabled list, reporting as it goes. One update runs at
     * a time in the app: a second waits for the first to end. It fails with
     * [AbdException.TimedOut] after [UPDATE_TIMEOUT].
     */
    fun update(): Flow<UpdateEvent>

    /** Downloads this list if it is enabled and is still missing a local copy. */
    fun updateList(url: String): Flow<UpdateEvent>

    /** Refreshes the root module's lists, including after cancelling a root handoff. */
    fun updateRoot(): Flow<UpdateEvent>
}

// A singleton, for the one lock: the background update, the Filters page and
// onboarding each start updates, and two abd updates at once replace each
// other's files.
@Singleton
internal class AbdListsRepository(
    private val abd: AbdClient,
    private val root: AbdClient,
    private val changes: DataChanges,
    private val userData: UserDataRepository,
) : ListsRepository {
    @Inject
    constructor(abd: AbdClient, root: RootTransport, changes: DataChanges, userData: UserDataRepository) : this(abd, root.fixedAbdClient(), changes, userData)

    private val lock = Mutex()
    private val running = MutableStateFlow(false)

    override val updating: StateFlow<Boolean> = running.asStateFlow()

    override suspend fun lists(): List<FilterList> = abd.lists()

    override suspend fun setEnabled(url: String, enabled: Boolean) = changes.changing { abd.setListEnabled(url, enabled) }

    override suspend fun configure(choices: Map<String, Boolean>) = changes.changing { abd.configureLists(choices) }

    override suspend fun add(url: String) = changes.changing { abd.addList(url) }

    override suspend fun remove(url: String) = changes.changing { abd.removeList(url) }

    override fun update(): Flow<UpdateEvent> = update(abd, rootOnly = false)

    override fun updateList(url: String): Flow<UpdateEvent> = update(abd, rootOnly = false, url = url)

    override fun updateRoot(): Flow<UpdateEvent> = update(root, rootOnly = true)

    private fun update(client: AbdClient, rootOnly: Boolean, url: String? = null): Flow<UpdateEvent> = flow {
        lock.withLock {
            val settings = userData.userData.first()
            if ((rootOnly || settings.method == FilterMethod.Root) && settings.hasPendingLists) {
                throw AbdException.CommandFailed("Subscription setup is pending")
            }
            running.value = true
            try {
                changes.changing {
                    withTimeoutOrNull(UPDATE_TIMEOUT) {
                        if (url == null) {
                            emitAll(client.update())
                        } else if (client.lists().any { it.url == url && it.enabled && it.rules == null }) {
                            emitAll(client.updateList(url))
                        }
                    } ?: throw AbdException.TimedOut()
                }
            } finally {
                running.value = false
            }
        }
    }
}
