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

package li.mercury.adblocker.feature.apps.impl.navigation

import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.navigation3.ListDetailSceneStrategy
import androidx.compose.ui.res.stringResource
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.core.ui.PaneSurface
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.feature.appdetail.api.navigation.AppDetailNavKey
import li.mercury.adblocker.feature.appdetail.api.navigation.navigateToAppDetail
import li.mercury.adblocker.feature.apps.api.navigation.AppsNavKey
import li.mercury.adblocker.feature.apps.impl.AppsScreen
import li.mercury.adblocker.feature.apps.impl.R
import li.mercury.adblocker.feature.settings.api.navigation.SettingsNavKey

/** The app list: on wide screens, the list pane of a list-detail layout. */
@OptIn(ExperimentalMaterial3AdaptiveApi::class)
fun EntryProviderScope<NavKey>.appsEntry(navigator: Navigator) {
    entry<AppsNavKey>(
        metadata = ListDetailSceneStrategy.listPane(
            detailPlaceholder = {
                PaneSurface { Placeholder(AdbIcons.Apps, stringResource(R.string.feature_apps_select), text = stringResource(R.string.feature_apps_select_text)) }
            },
        ),
    ) {
        val selected = navigator.state.currentSubStack.lastOrNull { it is AppDetailNavKey } as? AppDetailNavKey
        AppsScreen(
            selectedPackage = selected?.packageName,
            onOpenApp = navigator::navigateToAppDetail,
            onOpenSettings = { navigator.navigate(SettingsNavKey) },
            viewModel = hiltViewModel(),
        )
    }
}
