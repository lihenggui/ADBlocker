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

import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.WorkSelection
import timber.log.Timber
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Root setup deferred until the module can run. Magisk and APatch
 * start a new module at the next boot.
 */
@Singleton
class PendingSetup @Inject constructor(
    private val userData: UserDataRepository,
    private val module: ModuleRepository,
    private val apps: AppsRepository,
    private val lists: ListsRepository,
    private val methods: FilterMethodRepository,
    private val daemon: DaemonRepository,
) {
    private val applying = Mutex()

    /**
     * Retries a pending method switch and returns whether any deferred
     * onboarding choice was applied. Unapplied choices stay available for retry.
     */
    suspend fun applyIfReady(): Boolean = applying.withLock {
        var data = userData.userData.first()
        if (data.rootSwitchPending || data.rootHandoffCleanupPending) {
            methods.retryRootSwitch()
            data = userData.userData.first()
        }
        if (!data.onboardingDone || data.rootSwitchPending || data.method != FilterMethod.Root) return false
        if (data.pendingApps.isEmpty() && !data.hasPendingLists && data.pendingFilteringScope == null) return false
        if (module.state() != ModuleState.Ready) return false
        if (data.hasPendingLists) {
            val choices = data.pendingEnabledLists.associateWith { true } + data.pendingDisabledLists.associateWith { false }
            if (suspendRunCatching { lists.configure(choices) }.isFailure) {
                Timber.w("pending setup: retry subscription choices")
                return false
            }
        }
        // An empty, confirmed selection replaces earlier personal choices.
        // Work apps are kept if the setup inventory omitted them.
        if (suspendRunCatching {
                apps.configureDeferred(data.pendingApps, work = if (data.pendingPreserveWorkApps) WorkSelection.Keep else WorkSelection.Replace)
                data.pendingFilteringScope?.let { daemon.setFilteringScope(it) }
            }.isFailure
        ) {
            Timber.w("pending setup: retry app selection")
            return false
        }
        userData.removePending(data.pendingApps, data.pendingDisabledLists, data.pendingEnabledLists)
        true
    }
}
