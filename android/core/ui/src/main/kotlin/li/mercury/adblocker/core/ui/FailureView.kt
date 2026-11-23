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

package li.mercury.adblocker.core.ui

import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.LoadFailure

/** What a screen shows when its data could not be read: why, and what to do about it. */
@Composable
fun FailureView(failure: LoadFailure, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    val (title, text) = failureStrings(failure)
    Placeholder(
        icon = AdbIcons.Warning,
        title = stringResource(title),
        text = stringResource(text),
        modifier = modifier,
        action = { Button(onClick = onRetry) { Text(stringResource(R.string.core_ui_retry)) } },
    )
}

/** What can be done about [failure], in a sentence: what a screen says under the failure's title. */
@Composable
fun failureText(failure: LoadFailure): String = stringResource(failureStrings(failure).second)

private fun failureStrings(failure: LoadFailure): Pair<Int, Int> = when (failure) {
    LoadFailure.NoRoot -> R.string.core_ui_failure_no_root to R.string.core_ui_failure_no_root_text
    LoadFailure.ModuleMissing -> R.string.core_ui_failure_module_missing to R.string.core_ui_failure_module_missing_text
    LoadFailure.EngineMissing -> R.string.core_ui_failure_engine_missing to R.string.core_ui_failure_engine_missing_text
    LoadFailure.TimedOut -> R.string.core_ui_failure_timed_out to R.string.core_ui_failure_timed_out_text
    LoadFailure.Failed -> R.string.core_ui_failure_failed to R.string.core_ui_failure_failed_text
}
