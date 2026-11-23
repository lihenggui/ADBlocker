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

package li.mercury.adblocker.feature.filters.impl.navigation

import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.feature.filters.api.navigation.FiltersNavKey
import li.mercury.adblocker.feature.filters.api.navigation.RuleEditorNavKey
import li.mercury.adblocker.feature.filters.impl.FiltersScreen
import li.mercury.adblocker.feature.filters.impl.RuleEditorScreen
import li.mercury.adblocker.feature.settings.api.navigation.SettingsNavKey

fun EntryProviderScope<NavKey>.filtersEntries(navigator: Navigator) {
    entry<FiltersNavKey> {
        FiltersScreen(
            onOpenEditor = { navigator.navigate(RuleEditorNavKey) },
            onOpenSettings = { navigator.navigate(SettingsNavKey) },
            viewModel = hiltViewModel(),
        )
    }
    entry<RuleEditorNavKey> { key ->
        RuleEditorScreen(
            onBack = { if (navigator.state.currentKey == key) navigator.goBack() },
            viewModel = hiltViewModel(),
        )
    }
}
