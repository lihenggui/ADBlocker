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

package li.mercury.adblocker.feature.overview.impl.navigation

import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.feature.appdetail.api.navigation.navigateToAppDetail
import li.mercury.adblocker.feature.filters.api.navigation.FiltersNavKey
import li.mercury.adblocker.feature.https.api.navigation.HttpsNavKey
import li.mercury.adblocker.feature.overview.api.navigation.OverviewNavKey
import li.mercury.adblocker.feature.overview.impl.OverviewScreen
import li.mercury.adblocker.feature.settings.api.navigation.SettingsNavKey

fun EntryProviderScope<NavKey>.overviewEntry(navigator: Navigator) {
    entry<OverviewNavKey> {
        OverviewScreen(
            onOpenApp = navigator::navigateToAppDetail,
            onOpenFilters = { navigator.navigate(FiltersNavKey) },
            onOpenSettings = { navigator.navigate(SettingsNavKey) },
            onOpenHttps = { navigator.navigate(HttpsNavKey) },
            viewModel = hiltViewModel(),
        )
    }
}
