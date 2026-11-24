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

package li.mercury.adblocker.feature.onboarding.impl.steps

import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedToggleItem
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.categoryIcon
import li.mercury.adblocker.core.ui.categoryName
import li.mercury.adblocker.feature.onboarding.impl.OnboardingUiState
import li.mercury.adblocker.feature.onboarding.impl.R

@Composable
internal fun ColumnScope.Lists(state: OnboardingUiState, onToggle: (String) -> Unit, onNext: () -> Unit) {
    StepPage(
        title = stringResource(R.string.feature_onboarding_lists_title),
        text = stringResource(R.string.feature_onboarding_lists_text),
        art = null,
        // The module is installed next; the VPN asks for its grants first.
        action = {
            StepAction(
                stringResource(if (state.method == FilterMethod.Vpn) R.string.feature_onboarding_continue else R.string.feature_onboarding_step_start),
                onNext,
            )
        },
    ) {
        val choices = state.lists.keys.mapNotNull(FilterCatalog::byUrl)
        SegmentedGroup(choices.size) { index ->
            val entry = choices[index]
            val on = state.lists.getValue(entry.url)
            SegmentedToggleItem(
                index = index,
                count = choices.size,
                headline = entry.title,
                checked = on,
                onCheckedChange = { onToggle(entry.url) },
                leading = { Icon(categoryIcon(entry.category), contentDescription = null, Modifier.size(20.dp)) },
                supporting = { Text(stringResource(categoryName(entry.category))) },
                trailing = {
                    Switch(
                        checked = on,
                        onCheckedChange = null,
                        thumbContent = if (on) {
                            { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(SwitchDefaults.IconSize)) }
                        } else {
                            null
                        },
                    )
                },
            )
        }
        Hint(stringResource(R.string.feature_onboarding_lists_hint), top = 8.dp)
    }
}
