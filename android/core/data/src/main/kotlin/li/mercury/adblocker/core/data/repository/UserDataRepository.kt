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
import li.mercury.adblocker.core.datastore.UserPreferencesDataSource
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.model.UserData
import javax.inject.Inject

interface UserDataRepository {
    val userData: Flow<UserData>

    suspend fun setThemeMode(mode: ThemeMode)

    suspend fun setUseDynamicColor(use: Boolean)

    suspend fun setUpdateInterval(interval: UpdateInterval)

    suspend fun setUpdateOnWifiOnly(wifiOnly: Boolean)

    suspend fun setShowSystemApps(show: Boolean)

    suspend fun setOnboardingDone(done: Boolean)

    suspend fun dismissTip(key: String)

    /** What onboarding chose, to apply once the module runs; empty sets clear it. */
    suspend fun setPending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>, preserveWorkApps: Boolean = true, filteringScope: FilteringScope? = null)

    /** Clears the onboarding choices already applied. */
    suspend fun removePending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>)

    suspend fun setListPaneWide(wide: Boolean)

    suspend fun setRootSwitchPending()

    suspend fun setRootSwitchCancelling()

    /** Records the attempt before the root command can create a rollback snapshot. */
    suspend fun startRootHandoff(): String

    suspend fun rootHandoffAttempt(): String?

    suspend fun clearRootHandoffAttempt()

    /** Publishes root mode and saves the token for deleting the rollback snapshot. */
    suspend fun publishRootHandoff()

    suspend fun rootHandoffToFinish(): String?

    suspend fun clearRootHandoffToFinish()

    /** The root apps before any capture change, absent until saved. */
    suspend fun rootAppsBeforeSwitch(): Set<String>?

    /** Saves the original app selection once, before any root app writes. */
    suspend fun rememberRootAppsBeforeSwitch(apps: Set<String>)

    /** Publishes the active method and clears the pending switch and its saved apps together. */
    suspend fun setFilterMethod(method: FilterMethod)
}

internal class DataStoreUserDataRepository @Inject constructor(
    private val source: UserPreferencesDataSource,
) : UserDataRepository {
    override val userData: Flow<UserData> = source.userData

    override suspend fun setThemeMode(mode: ThemeMode) {
        source.setThemeMode(mode)
    }

    override suspend fun setUseDynamicColor(use: Boolean) {
        source.setUseDynamicColor(use)
    }

    override suspend fun setUpdateInterval(interval: UpdateInterval) {
        source.setUpdateInterval(interval)
    }

    override suspend fun setUpdateOnWifiOnly(wifiOnly: Boolean) {
        source.setUpdateOnWifiOnly(wifiOnly)
    }

    override suspend fun setShowSystemApps(show: Boolean) {
        source.setShowSystemApps(show)
    }

    override suspend fun setOnboardingDone(done: Boolean) {
        source.setOnboardingDone(done)
    }

    override suspend fun dismissTip(key: String) {
        source.dismissTip(key)
    }

    override suspend fun setPending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>, preserveWorkApps: Boolean, filteringScope: FilteringScope?) {
        source.setPending(apps, disabledLists, enabledLists, preserveWorkApps, filteringScope)
    }

    override suspend fun removePending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>) {
        source.removePending(apps, disabledLists, enabledLists)
    }

    override suspend fun setListPaneWide(wide: Boolean) {
        source.setListPaneWide(wide)
    }

    override suspend fun setRootSwitchPending() {
        source.setRootSwitchPending()
    }

    override suspend fun setRootSwitchCancelling() {
        source.setRootSwitchCancelling()
    }

    override suspend fun startRootHandoff() = source.startRootHandoff()

    override suspend fun rootHandoffAttempt() = source.rootHandoffAttempt()

    override suspend fun clearRootHandoffAttempt() {
        source.clearRootHandoffAttempt()
    }

    override suspend fun publishRootHandoff() {
        source.publishRootHandoff()
    }

    override suspend fun rootHandoffToFinish() = source.rootHandoffToFinish()

    override suspend fun clearRootHandoffToFinish() {
        source.clearRootHandoffToFinish()
    }

    override suspend fun rootAppsBeforeSwitch() = source.rootAppsBeforeSwitch()

    override suspend fun rememberRootAppsBeforeSwitch(apps: Set<String>) {
        source.rememberRootAppsBeforeSwitch(apps)
    }

    override suspend fun setFilterMethod(method: FilterMethod) {
        source.setFilterMethod(method)
    }
}
