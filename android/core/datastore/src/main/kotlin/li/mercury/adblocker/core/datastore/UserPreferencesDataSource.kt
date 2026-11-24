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

package li.mercury.adblocker.core.datastore

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.MutablePreferences
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.emptyPreferences
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ThemeMode
import li.mercury.adblocker.core.model.UpdateInterval
import li.mercury.adblocker.core.model.UserData
import timber.log.Timber
import java.io.IOException
import java.util.UUID
import javax.inject.Inject

/**
 * The user's settings, stored in the app's own data. Nothing here describes
 * what apps connect to: that stays with the daemon, readable by root only.
 */
class UserPreferencesDataSource @Inject constructor(
    private val dataStore: DataStore<Preferences>,
) {
    val userData: Flow<UserData> = dataStore.data
        // A file that can't be read is treated as unset rather than failing
        // every screen that follows the settings. An unreadable (corrupt) file
        // is replaced before it gets here (DataStoreModule); this is the disk.
        .catch { e ->
            if (e !is IOException) throw e
            Timber.w("settings: not read: %s", e.javaClass.simpleName)
            emit(emptyPreferences())
        }
        .map { preferences ->
            val defaults = UserData()
            UserData(
                themeMode = preferences[THEME_MODE].toEnum(defaults.themeMode),
                useDynamicColor = preferences[USE_DYNAMIC_COLOR] ?: defaults.useDynamicColor,
                updateInterval = preferences[UPDATE_INTERVAL].toEnum(defaults.updateInterval),
                updateOnWifiOnly = preferences[UPDATE_ON_WIFI_ONLY] ?: defaults.updateOnWifiOnly,
                showSystemApps = preferences[SHOW_SYSTEM_APPS] ?: defaults.showSystemApps,
                onboardingDone = preferences[ONBOARDING_DONE] ?: defaults.onboardingDone,
                dismissedTips = preferences[DISMISSED_TIPS] ?: defaults.dismissedTips,
                pendingApps = preferences[PENDING_APPS] ?: defaults.pendingApps,
                pendingFilteringScope = preferences[PENDING_FILTERING_SCOPE]?.let { name -> FilteringScope.entries.firstOrNull { it.name == name } },
                pendingPreserveWorkApps = preferences[PENDING_PRESERVE_WORK_APPS] ?: defaults.pendingPreserveWorkApps,
                pendingDisabledLists = preferences[PENDING_DISABLED_LISTS] ?: defaults.pendingDisabledLists,
                pendingEnabledLists = preferences[PENDING_ENABLED_LISTS] ?: defaults.pendingEnabledLists,
                listPaneWide = preferences[LIST_PANE_WIDE] ?: defaults.listPaneWide,
                filterMethod = preferences[FILTER_METHOD]?.let { name -> FilterMethod.entries.firstOrNull { it.name == name } },
                rootSwitchPending = preferences[ROOT_SWITCH_PENDING] ?: defaults.rootSwitchPending,
                rootSwitchCancelling = preferences[ROOT_SWITCH_CANCELLING] ?: defaults.rootSwitchCancelling,
                rootHandoffCleanupPending = preferences[ROOT_HANDOFF_TO_FINISH] != null,
                moduleInstalledByApp = preferences[MODULE_INSTALLED_BY_APP] ?: defaults.moduleInstalledByApp,
            )
        }

    suspend fun setThemeMode(mode: ThemeMode) = dataStore.edit { it[THEME_MODE] = mode.name }

    suspend fun setUseDynamicColor(use: Boolean) = dataStore.edit { it[USE_DYNAMIC_COLOR] = use }

    suspend fun setUpdateInterval(interval: UpdateInterval) = dataStore.edit { it[UPDATE_INTERVAL] = interval.name }

    suspend fun setUpdateOnWifiOnly(wifiOnly: Boolean) = dataStore.edit { it[UPDATE_ON_WIFI_ONLY] = wifiOnly }

    suspend fun setShowSystemApps(show: Boolean) = dataStore.edit { it[SHOW_SYSTEM_APPS] = show }

    suspend fun setOnboardingDone(done: Boolean) = dataStore.edit { it[ONBOARDING_DONE] = done }

    suspend fun dismissTip(key: String) = dataStore.edit { it[DISMISSED_TIPS] = (it[DISMISSED_TIPS] ?: emptySet()) + key }

    suspend fun setListPaneWide(wide: Boolean) = dataStore.edit { it[LIST_PANE_WIDE] = wide }

    suspend fun setRootSwitchPending() = dataStore.edit { it[ROOT_SWITCH_PENDING] = true }

    suspend fun setRootSwitchCancelling() = dataStore.edit { it[ROOT_SWITCH_CANCELLING] = true }

    suspend fun setModuleInstalledByApp(installed: Boolean) = dataStore.edit { it[MODULE_INSTALLED_BY_APP] = installed }

    suspend fun startRootHandoff(): String = checkNotNull(
        dataStore.edit {
            if (it[ROOT_HANDOFF_ATTEMPT] == null) it[ROOT_HANDOFF_ATTEMPT] = UUID.randomUUID().toString()
        }[ROOT_HANDOFF_ATTEMPT],
    )

    suspend fun rootHandoffAttempt(): String? = dataStore.data.first()[ROOT_HANDOFF_ATTEMPT]

    suspend fun clearRootHandoffAttempt() = dataStore.edit { it.remove(ROOT_HANDOFF_ATTEMPT) }

    suspend fun publishRootHandoff() = dataStore.edit {
        it[ROOT_HANDOFF_TO_FINISH] = checkNotNull(it[ROOT_HANDOFF_ATTEMPT])
        it.useMethod(FilterMethod.Root)
    }

    suspend fun rootHandoffToFinish(): String? = dataStore.data.first()[ROOT_HANDOFF_TO_FINISH]

    suspend fun clearRootHandoffToFinish() = dataStore.edit { it.remove(ROOT_HANDOFF_TO_FINISH) }

    /** A missing key means no root app writes began. An empty set is a saved selection. */
    suspend fun rootAppsBeforeSwitch(): Set<String>? = dataStore.data.first()[ROOT_APPS_BEFORE_SWITCH]

    suspend fun rememberRootAppsBeforeSwitch(apps: Set<String>) = dataStore.edit {
        if (it[ROOT_APPS_BEFORE_SWITCH] == null) {
            it[ROOT_APPS_BEFORE_SWITCH] = apps
        }
    }

    suspend fun setFilterMethod(method: FilterMethod) = dataStore.edit { it.useMethod(method) }

    private fun MutablePreferences.useMethod(method: FilterMethod) {
        this[FILTER_METHOD] = method.name
        remove(ROOT_SWITCH_PENDING)
        remove(ROOT_SWITCH_CANCELLING)
        remove(ROOT_HANDOFF_ATTEMPT)
        remove(ROOT_APPS_BEFORE_SWITCH)
    }

    suspend fun setPending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>, preserveWorkApps: Boolean, filteringScope: FilteringScope?) = dataStore.edit {
        it[PENDING_APPS] = apps
        if (filteringScope == null) it.remove(PENDING_FILTERING_SCOPE) else it[PENDING_FILTERING_SCOPE] = filteringScope.name
        it[PENDING_PRESERVE_WORK_APPS] = preserveWorkApps
        it[PENDING_DISABLED_LISTS] = disabledLists
        it[PENDING_ENABLED_LISTS] = enabledLists
    }

    suspend fun removePending(apps: Set<String>, disabledLists: Set<String>, enabledLists: Set<String>) = dataStore.edit {
        it[PENDING_APPS] = it[PENDING_APPS].orEmpty() - apps
        it.remove(PENDING_FILTERING_SCOPE)
        it[PENDING_DISABLED_LISTS] = it[PENDING_DISABLED_LISTS].orEmpty() - disabledLists
        it[PENDING_ENABLED_LISTS] = it[PENDING_ENABLED_LISTS].orEmpty() - enabledLists
    }

    private companion object {
        val THEME_MODE = stringPreferencesKey("theme_mode")
        val USE_DYNAMIC_COLOR = booleanPreferencesKey("use_dynamic_color")
        val UPDATE_INTERVAL = stringPreferencesKey("update_interval")
        val UPDATE_ON_WIFI_ONLY = booleanPreferencesKey("update_on_wifi_only")
        val SHOW_SYSTEM_APPS = booleanPreferencesKey("show_system_apps")
        val ONBOARDING_DONE = booleanPreferencesKey("onboarding_done")
        val DISMISSED_TIPS = stringSetPreferencesKey("dismissed_tips")
        val PENDING_FILTERING_SCOPE = stringPreferencesKey("pending_filtering_scope")
        val PENDING_APPS = stringSetPreferencesKey("pending_apps")
        val PENDING_PRESERVE_WORK_APPS = booleanPreferencesKey("pending_preserve_work_apps")
        val PENDING_DISABLED_LISTS = stringSetPreferencesKey("pending_disabled_lists")
        val PENDING_ENABLED_LISTS = stringSetPreferencesKey("pending_enabled_lists")
        val LIST_PANE_WIDE = booleanPreferencesKey("list_pane_wide")
        val FILTER_METHOD = stringPreferencesKey("filter_method")
        val ROOT_SWITCH_PENDING = booleanPreferencesKey("root_switch_pending")
        val ROOT_SWITCH_CANCELLING = booleanPreferencesKey("root_switch_cancelling")
        val ROOT_APPS_BEFORE_SWITCH = stringSetPreferencesKey("root_apps_before_switch")
        val ROOT_HANDOFF_ATTEMPT = stringPreferencesKey("root_handoff_attempt")
        val ROOT_HANDOFF_TO_FINISH = stringPreferencesKey("root_handoff_to_finish")
        val MODULE_INSTALLED_BY_APP = booleanPreferencesKey("module_installed_by_app")
    }
}

/** A stored enum name, or [default] for one unset or no longer known. */
private inline fun <reified E : Enum<E>> String?.toEnum(default: E): E = enumValues<E>().firstOrNull { it.name == this } ?: default
