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

package li.mercury.adblocker.feature.activity.impl.navigation

import androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi
import androidx.compose.material3.adaptive.navigation3.ListDetailSceneStrategy
import androidx.compose.ui.res.stringResource
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.navigation.BottomSheetSceneStrategy
import li.mercury.adblocker.core.navigation.Navigator
import li.mercury.adblocker.core.ui.PaneSurface
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.feature.activity.api.navigation.ActivityNavKey
import li.mercury.adblocker.feature.activity.api.navigation.RequestNavKey
import li.mercury.adblocker.feature.activity.impl.ActivityScreen
import li.mercury.adblocker.feature.activity.impl.R
import li.mercury.adblocker.feature.activity.impl.RequestScreen
import li.mercury.adblocker.feature.activity.impl.RequestViewModel
import li.mercury.adblocker.feature.settings.api.navigation.SettingsNavKey

/**
 * The recorded requests as a list pane, and a request as its detail: beside the
 * list on wide screens, in a modal bottom sheet over it on others.
 */
@OptIn(ExperimentalMaterial3AdaptiveApi::class)
fun EntryProviderScope<NavKey>.activityEntries(navigator: Navigator) {
    entry<ActivityNavKey>(
        metadata = ListDetailSceneStrategy.listPane(
            detailPlaceholder = {
                PaneSurface { Placeholder(AdbIcons.ReceiptLong, stringResource(R.string.feature_activity_select), text = stringResource(R.string.feature_activity_select_text)) }
            },
        ),
    ) {
        val selected = navigator.state.currentSubStack.lastOrNull() as? RequestNavKey
        ActivityScreen(
            selectedId = selected?.id,
            onOpenRequest = { event ->
                navigator.state.currentSubStack.removeAll { it is RequestNavKey }
                navigator.navigate(RequestNavKey(event.id))
            },
            onOpenSettings = { navigator.navigate(SettingsNavKey) },
            viewModel = hiltViewModel(),
        )
    }
    entry<RequestNavKey>(metadata = ListDetailSceneStrategy.detailPane() + BottomSheetSceneStrategy.bottomSheet()) { key ->
        RequestScreen(
            onClose = navigator::goBack,
            viewModel = hiltViewModel<RequestViewModel, RequestViewModel.Factory>(key = key.id.toString()) { it.create(key.id) },
        )
    }
}
