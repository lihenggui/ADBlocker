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

package li.mercury.adblocker.feature.appdetail.impl.navigation

import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.navigation3.ListDetailSceneStrategy
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.feature.activity.api.navigation.ActivityNavKey
import li.mercury.adblocker.feature.appdetail.api.navigation.AppDetailNavKey
import li.mercury.adblocker.feature.appdetail.impl.AppDetailScreen
import li.mercury.adblocker.feature.appdetail.impl.AppDetailViewModel
import li.mercury.adblocker.feature.https.api.navigation.HttpsNavKey
import li.mercury.adblocker.feature.https.api.navigation.WorkHttpsNavKey

/** An app's page: the detail pane beside the app list on wide screens. */
@OptIn(ExperimentalMaterial3AdaptiveApi::class)
fun EntryProviderScope<NavKey>.appDetailEntry(navigator: Navigator) {
    entry<AppDetailNavKey>(metadata = ListDetailSceneStrategy.detailPane()) { key ->
        AppDetailScreen(
            onBack = navigator::goBack,
            onOpenActivity = { navigator.navigate(ActivityNavKey) },
            onOpenHttps = { navigator.navigate(HttpsNavKey) },
            onOpenWorkHttps = { navigator.navigate(WorkHttpsNavKey) },
            viewModel = hiltViewModel<AppDetailViewModel, AppDetailViewModel.Factory>(key = key.packageName) { it.create(key.packageName) },
        )
    }
}
