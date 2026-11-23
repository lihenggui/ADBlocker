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

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.job
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.datastore.UserPreferencesDataSource
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.model.UserData
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import kotlin.test.Test
import kotlin.test.assertEquals

class DataStoreUserDataRepositoryTest {
    @get:Rule
    val temporary = TemporaryFolder()

    @Test
    fun givenAnEmptyStore_whenSettingsAreRead_thenTheRepositoryReturnsDefaults() = runTest {
        withRepository { repository, _ ->
            val settings = repository.userData.first()
            assertEquals(UserData(), settings)
            assertEquals(FilterMethod.Root, settings.method)
        }
    }

    @Test
    fun givenUnknownStoredEnums_whenSettingsAreRead_thenOnlyThoseSettingsFallBack() = runTest {
        withRepository { repository, store ->
            store.edit {
                it[stringPreferencesKey("theme_mode")] = "OldTheme"
                it[stringPreferencesKey("update_interval")] = "OldInterval"
                it[stringPreferencesKey("filter_method")] = "OldMethod"
                it[stringPreferencesKey("pending_filtering_scope")] = "OldScope"
            }
            repository.setShowSystemApps(true)

            assertEquals(UserData(showSystemApps = true), repository.userData.first())
        }
    }

    @Test
    fun givenAppearanceAndUpdateSettingsWereChanged_whenTheStoreReopens_thenAllSettingsAreRestoredTogether() = runTest {
        withRepository { repository, _ ->
            repository.setThemeMode(ThemeMode.Dark)
            repository.setUseDynamicColor(false)
            repository.setUpdateInterval(UpdateInterval.Weekly)
            repository.setUpdateOnWifiOnly(false)
            repository.setShowSystemApps(true)
            repository.setOnboardingDone(true)
            repository.dismissTip("root")
            repository.dismissTip("vpn")
            repository.setListPaneWide(true)
        }

        withRepository { repository, _ ->
            assertEquals(
                UserData(
                    themeMode = ThemeMode.Dark,
                    useDynamicColor = false,
                    updateInterval = UpdateInterval.Weekly,
                    updateOnWifiOnly = false,
                    showSystemApps = true,
                    onboardingDone = true,
                    dismissedTips = setOf("root", "vpn"),
                    listPaneWide = true,
                ),
                repository.userData.first(),
            )
        }
    }

    @Test
    fun givenDeferredChoices_whenSomeAreCleared_thenOnlyUnappliedChoicesRemain() = runTest {
        val applied = "com.example.news"
        val pending = "37:com.example.news"
        withRepository { repository, _ ->
            repository.setPending(setOf(applied, pending), setOf("disabled"), setOf("enabled"), false, FilteringScope.All)
            assertEquals(FilteringScope.All, repository.userData.first().pendingFilteringScope)
            repository.removePending(setOf(applied), setOf("disabled"), emptySet())
        }

        withRepository { repository, _ ->
            assertEquals(
                UserData(
                    pendingApps = setOf(pending),
                    pendingPreserveWorkApps = false,
                    pendingEnabledLists = setOf("enabled"),
                ),
                repository.userData.first(),
            )
        }
    }

    private suspend fun withRepository(action: suspend (DataStoreUserDataRepository, DataStore<Preferences>) -> Unit) {
        val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
        val store = PreferenceDataStoreFactory.create(scope = scope) { temporary.root.resolve("user.preferences_pb") }
        try {
            action(DataStoreUserDataRepository(UserPreferencesDataSource(store)), store)
        } finally {
            scope.coroutineContext.job.cancelAndJoin()
        }
    }
}
