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

package li.mercury.adblocker.core.testing

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.model.UserData
import java.util.UUID

/** Settings held in memory, starting from [initial]. */
class TestUserDataRepository(initial: UserData = UserData()) : UserDataRepository {
    override val userData = MutableStateFlow(initial)
    private var originalRootApps: Set<String>? = null
    private var handoffAttempt: String? = null
    private var handoffToFinish: String? = null

    override suspend fun setThemeMode(mode: ThemeMode) = userData.update { it.copy(themeMode = mode) }

    override suspend fun setUseDynamicColor(use: Boolean) = userData.update { it.copy(useDynamicColor = use) }

    override suspend fun setUpdateInterval(interval: UpdateInterval) = userData.update { it.copy(updateInterval = interval) }

    override suspend fun setUpdateOnWifiOnly(wifiOnly: Boolean) = userData.update { it.copy(updateOnWifiOnly = wifiOnly) }

    override suspend fun setShowSystemApps(show: Boolean) = userData.update { it.copy(showSystemApps = show) }

    override suspend fun setOnboardingDone(done: Boolean) = userData.update { it.copy(onboardingDone = done) }

    override suspend fun dismissTip(key: String) = userData.update { it.copy(dismissedTips = it.dismissedTips + key) }

    override suspend fun setPending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>, preserveWorkApps: Boolean, filteringScope: FilteringScope?) = userData.update {
        it.copy(
            pendingApps = apps,
            pendingFilteringScope = filteringScope,
            pendingPreserveWorkApps = preserveWorkApps,
            pendingDisabledLists = disabledLists,
            pendingEnabledLists = enabledLists,
        )
    }

    override suspend fun removePending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>) = userData.update {
        it.copy(
            pendingApps = it.pendingApps - apps,
            pendingFilteringScope = null,
            pendingDisabledLists = it.pendingDisabledLists - disabledLists,
            pendingEnabledLists = it.pendingEnabledLists - enabledLists,
        )
    }

    override suspend fun setListPaneWide(wide: Boolean) = userData.update { it.copy(listPaneWide = wide) }

    override suspend fun setRootSwitchPending() = userData.update { it.copy(rootSwitchPending = true) }

    override suspend fun setRootSwitchCancelling() = userData.update { it.copy(rootSwitchCancelling = true) }

    override suspend fun setModuleInstalledByApp(installed: Boolean) = userData.update { it.copy(moduleInstalledByApp = installed) }

    override suspend fun startRootHandoff(): String = handoffAttempt ?: UUID.randomUUID().toString().also { handoffAttempt = it }

    override suspend fun rootHandoffAttempt() = handoffAttempt

    override suspend fun clearRootHandoffAttempt() {
        handoffAttempt = null
    }

    override suspend fun publishRootHandoff() {
        handoffToFinish = checkNotNull(handoffAttempt)
        setFilterMethod(FilterMethod.Root)
    }

    override suspend fun rootHandoffToFinish() = handoffToFinish

    override suspend fun clearRootHandoffToFinish() {
        handoffToFinish = null
        userData.update { it.copy(rootHandoffCleanupPending = false) }
    }

    override suspend fun rootAppsBeforeSwitch() = originalRootApps

    override suspend fun rememberRootAppsBeforeSwitch(apps: Set<String>) {
        if (originalRootApps == null) originalRootApps = apps
    }

    override suspend fun setFilterMethod(method: FilterMethod) {
        originalRootApps = null
        handoffAttempt = null
        userData.update {
            it.copy(
                filterMethod = method,
                rootSwitchPending = false,
                rootSwitchCancelling = false,
                rootHandoffCleanupPending = handoffToFinish != null,
            )
        }
    }
}
