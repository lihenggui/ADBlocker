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

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.model.FilteringScope

@Composable
fun FilteringScopeChoice(selected: FilteringScope, onSelect: (FilteringScope) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Column(modifier.fillMaxWidth()) {
        SectionHeader(stringResource(R.string.core_ui_filtering_scope))
        ConnectedChoice(
            options = FilteringScope.entries,
            selected = selected,
            onSelect = onSelect,
            label = { stringResource(if (it == FilteringScope.All) R.string.core_ui_scope_all else R.string.core_ui_scope_chosen) },
            enabled = enabled,
        )
        Reveal(selected == FilteringScope.All) {
            Text(
                stringResource(R.string.core_ui_scope_all_text),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            )
        }
    }
}
