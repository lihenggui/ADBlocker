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

package li.mercury.adblocker.feature.filters.impl

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.ui.LocalCompactWindow
import li.mercury.adblocker.core.ui.SectionHeader
import li.mercury.adblocker.core.ui.SegmentedGroup
import li.mercury.adblocker.core.ui.SegmentedItem
import li.mercury.adblocker.core.ui.animatedRow
import li.mercury.adblocker.core.ui.pageMargin
import li.mercury.adblocker.core.ui.segmentedItems

@Composable
internal fun TrustedTab(state: FiltersUiState, actions: FiltersActions, onAdd: () -> Unit) {
    val margin = pageMargin(LocalCompactWindow.current)
    val motion = MaterialTheme.motionScheme
    LazyColumn(contentPadding = PaddingValues(start = margin, end = margin, top = 12.dp, bottom = 24.dp), modifier = Modifier.fillMaxSize()) {
        item(key = "text") {
            Text(
                stringResource(R.string.feature_filters_trusted_text),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.animatedRow(this, motion).padding(horizontal = 16.dp),
            )
        }
        item(key = "header") {
            SectionHeader(stringResource(R.string.feature_filters_tab_trusted), Modifier.animatedRow(this, motion), trailing = state.trusted.size.toString())
        }
        // A row an item: a site removed leaves as the others close up.
        segmentedItems(state.trusted, key = { "site-$it" }) { index, count, host -> TrustedRow(host, index, count, actions.untrust) }
        item(key = "add") {
            FilledTonalButton(onClick = onAdd, modifier = Modifier.animatedRow(this, motion).padding(top = 16.dp)) {
                Icon(AdbIcons.Add, contentDescription = null, Modifier.size(ButtonDefaults.IconSize))
                Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                Text(stringResource(R.string.feature_filters_add_trusted))
            }
        }
    }
}

/** Trusted hosts, in monospace as hosts are everywhere in the app. */
@Composable
internal fun TrustedGroup(hosts: List<String>, onRemove: (String) -> Unit, modifier: Modifier = Modifier) {
    SegmentedGroup(hosts.size, modifier) { index -> TrustedRow(hosts[index], index, hosts.size, onRemove) }
}

@Composable
private fun TrustedRow(host: String, index: Int, count: Int, onRemove: (String) -> Unit) {
    SegmentedItem(
        index = index,
        count = count,
        headline = host,
        headlineStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
        leading = { Icon(AdbIcons.Public, contentDescription = null, Modifier.size(20.dp)) },
        trailing = {
            IconButton(onClick = { onRemove(host) }) {
                Icon(AdbIcons.Delete, contentDescription = stringResource(R.string.feature_filters_remove_trusted, host))
            }
        },
    )
}
