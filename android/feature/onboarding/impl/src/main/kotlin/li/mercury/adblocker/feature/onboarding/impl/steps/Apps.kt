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

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.ui.AppIcon
import li.mercury.adblocker.core.ui.AppListWithheld
import li.mercury.adblocker.core.ui.ContentKind
import li.mercury.adblocker.core.ui.FadeThrough
import li.mercury.adblocker.core.ui.FilteringScopeChoice
import li.mercury.adblocker.core.ui.Placeholder
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedToggleItem
import li.mercury.adblocker.core.ui.StepAction
import li.mercury.adblocker.core.ui.StepPage
import li.mercury.adblocker.core.ui.message
import li.mercury.adblocker.core.ui.profileAppLabel
import li.mercury.adblocker.feature.onboarding.impl.OnboardingUiState
import li.mercury.adblocker.feature.onboarding.impl.R
import li.mercury.adblocker.core.ui.R as UiR

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun ColumnScope.Apps(
    state: OnboardingUiState,
    onToggle: (String) -> Unit,
    onFilteringScope: (FilteringScope) -> Unit,
    onNext: () -> Unit,
    onRetry: () -> Unit,
    onAllowAppList: () -> Unit,
) {
    var query by rememberSaveable { mutableStateOf("") }
    val all = state.filteringScope == FilteringScope.All
    val choices = if (all) state.apps.filter { it.profileSerial != null } else state.apps
    val canSearchGroups = choices.any { it.isSharedGroup }
    StepPage(
        title = stringResource(R.string.feature_onboarding_apps_title),
        text = if (all) null else stringResource(R.string.feature_onboarding_apps_text),
        art = null,
        action = {
            if (state.personalEligibilityUnavailable) {
                StepAction(stringResource(R.string.feature_onboarding_try_again), onRetry, enabled = !state.appsLoading)
            } else if (all) {
                StepAction(stringResource(R.string.feature_onboarding_continue), onNext, enabled = !state.appsLoading)
            } else {
                StepAction(pluralStringResource(R.plurals.feature_onboarding_apps_continue, state.picks.size, state.picks.size), onNext, enabled = !state.appsLoading)
            }
        },
    ) {
        FilteringScopeChoice(state.filteringScope, onFilteringScope)
        // All apps needs no choice of personal apps, so it goes on without the list.
        val kind = when {
            state.appsLoading -> ContentKind.Loading
            !all && state.appListWithheld -> ContentKind.Failure
            !all && choices.isEmpty() -> ContentKind.Empty
            else -> ContentKind.Content
        }
        FadeThrough(kind, { it }, Modifier.fillMaxWidth(), label = "apps") { shown ->
            when (shown) {
                ContentKind.Loading -> Box(Modifier.fillMaxWidth().padding(top = 48.dp), contentAlignment = Alignment.Center) { LoadingIndicator() }

                ContentKind.Failure -> AppListWithheld(onAllowAppList, onRetry, Modifier.padding(top = 8.dp))

                // A list that could not be read is not an empty one.
                ContentKind.Empty -> if (state.personalEligibilityUnavailable) {
                    EligibilityUnavailable()
                } else {
                    Placeholder(
                        icon = AdbIcons.Apps,
                        title = stringResource(R.string.feature_onboarding_apps_none),
                        text = stringResource(R.string.feature_onboarding_apps_none_text),
                        modifier = Modifier.padding(top = 8.dp),
                        prominent = true,
                    )
                }

                else -> Column {
                    if (canSearchGroups) {
                        OutlinedTextField(
                            value = query,
                            onValueChange = { query = it },
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
                            singleLine = true,
                            label = { Text(stringResource(R.string.feature_onboarding_search_apps)) },
                        )
                    }
                    if (state.personalEligibilityUnavailable) EligibilityUnavailable()
                    AppChoiceList(choices, state.picks, canSearchGroups, query, onToggle)
                }
            }
        }
    }
}

@Composable
private fun EligibilityUnavailable() {
    Text(
        stringResource(UiR.string.core_ui_personal_eligibility_unavailable),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 8.dp),
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AppChoiceList(
    choices: List<InstalledApp>,
    picks: Set<String>,
    canSearchGroups: Boolean,
    query: String,
    onToggle: (String) -> Unit,
) {
    val search = query.trim()
    val shown = if (!canSearchGroups || search.isEmpty()) {
        choices
    } else {
        choices.filter { app ->
            app.label.contains(search, ignoreCase = true) || app.packageName.contains(search, ignoreCase = true) ||
                app.sharedMembers.any { it.label.contains(search, ignoreCase = true) || it.packageName.contains(search, ignoreCase = true) }
        }
    }
    SegmentedGroup(shown.size) { index ->
        val app = shown[index]
        key(app.appKey) {
            val picked = app.appKey in picks
            val enabled = app.canEnableFiltering
            val unavailable = app.unavailableReason
            val label = profileAppLabel(app.label, app.profileSerial != null, app.isSharedGroup)
            // The choice is made now and applied when installing.
            SegmentedToggleItem(
                index = index,
                count = shown.size,
                headline = label,
                checked = picked,
                onCheckedChange = { onToggle(app.appKey) },
                supporting = if (unavailable == null && app.sharedMembers.isEmpty()) {
                    null
                } else {
                    {
                        Column {
                            unavailable?.let { Text(stringResource(it.message())) }
                            if (app.sharedMembers.isNotEmpty()) {
                                Text(stringResource(UiR.string.core_ui_shared_app_members, app.sharedMembers.joinToString { it.label }))
                            }
                        }
                    }
                },
                leading = { AppIcon(app, label) },
                showsSelection = true,
                enabled = enabled,
                trailing = { Checkbox(checked = picked, onCheckedChange = null, enabled = enabled) },
            )
        }
    }
}
