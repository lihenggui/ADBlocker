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

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemColors
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The page margin: 16dp in compact windows, 24dp from medium up (M3 layout,
 * breakpoints). Screens get it from the window and pass it down.
 */
fun pageMargin(compact: Boolean): Dp = if (compact) 16.dp else 24.dp

/** Where a page in a single pane stops growing, centered: the expanded breakpoint. Lines of text and rows wider than this are hard to read. */
val MaxContentWidth: Dp = 840.dp

/**
 * Segmented list item colors: the items on surface container, over the
 * surface of the page, as in the Compose SegmentedListItem samples, so that
 * the 2dp gaps between them show. A disabled item keeps its container, and
 * with it its place in the group; only its content is dimmed.
 */
@Composable
fun segmentedColors(): ListItemColors = ListItemDefaults.segmentedColors(
    containerColor = MaterialTheme.colorScheme.surfaceContainer,
    disabledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
)

/**
 * A group of segmented list items: 16dp outer and 4dp inner corners, 2dp
 * apart (M3 expressive lists). [item] is called with each item's index and
 * the count, from which [segmentedShapes] gives its shape.
 */
@Composable
fun SegmentedGroup(count: Int, modifier: Modifier = Modifier, item: @Composable (index: Int) -> Unit) {
    // A row that joins or leaves, or grows, moves what is below rather than making it jump.
    Column(modifier.fillMaxWidth().animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec()), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        for (i in 0 until count) item(i)
    }
}

/**
 * A [SegmentedGroup] as items of a lazy list, one item a row, so that a row
 * that joins or leaves the group fades while the others move to make room.
 */
fun <T> LazyListScope.segmentedItems(items: List<T>, key: (T) -> Any, row: @Composable (index: Int, count: Int, item: T) -> Unit) {
    itemsIndexed(items, key = { _, item -> key(item) }) { index, item ->
        // The gaps are between rows, as in a group: none after the last.
        val gap = if (index < items.lastIndex) ListItemDefaults.SegmentedGap else 0.dp
        Box(Modifier.animatedRow(this, MaterialTheme.motionScheme).padding(bottom = gap)) { row(index, items.size, item) }
    }
}

/** A list item of a [SegmentedGroup] that opens something, or does nothing if [onClick] is null. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SegmentedItem(
    index: Int,
    count: Int,
    headline: String,
    modifier: Modifier = Modifier,
    supporting: (@Composable () -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    selected: Boolean = false,
    /** The headline's style when it is not the list item's own, such as monospace for a host. */
    headlineStyle: TextStyle? = null,
    onClick: (() -> Unit)? = null,
) {
    val shapes = ListItemDefaults.segmentedShapes(index, count)
    val content: @Composable () -> Unit = {
        if (headlineStyle == null) Text(headline, maxLines = 1, overflow = TextOverflow.Ellipsis) else Text(headline, maxLines = 1, overflow = TextOverflow.Ellipsis, style = headlineStyle)
    }
    when {
        onClick == null -> SegmentedListItem(
            shapes = shapes,
            modifier = modifier,
            leadingContent = leading,
            trailingContent = trailing,
            supportingContent = supporting,
            colors = segmentedColors(),
            content = content,
        )

        else -> SegmentedListItem(
            selected = selected,
            onClick = onClick,
            shapes = shapes,
            modifier = modifier,
            leadingContent = leading,
            trailingContent = trailing,
            supportingContent = supporting,
            colors = segmentedColors(),
            content = content,
        )
    }
}

/**
 * A list item of a [SegmentedGroup] whose whole row toggles a switch or
 * checkbox in [trailing].
 *
 * Only a row that is chosen among others ([showsSelection]: the apps to
 * filter, picked from a list) takes M3's selected color and shape. A row that
 * holds a setting keeps its look: its switch says whether it is on.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SegmentedToggleItem(
    index: Int,
    count: Int,
    headline: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    supporting: (@Composable () -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    showsSelection: Boolean = false,
    enabled: Boolean = true,
    headlineLines: Int = 1,
    trailing: @Composable () -> Unit,
) {
    val shapes = ListItemDefaults.segmentedShapes(index, count)
    SegmentedListItem(
        checked = checked,
        onCheckedChange = onCheckedChange,
        enabled = enabled,
        shapes = if (showsSelection) shapes else shapes.copy(selectedShape = shapes.shape),
        modifier = modifier,
        leadingContent = leading,
        trailingContent = trailing,
        supportingContent = supporting,
        colors = if (showsSelection) segmentedColors() else settingColors(),
        content = { Text(headline, maxLines = headlineLines, overflow = TextOverflow.Ellipsis) },
    )
}

@Composable
private fun settingColors(): ListItemColors {
    val scheme = MaterialTheme.colorScheme
    return ListItemDefaults.segmentedColors(
        containerColor = scheme.surfaceContainer,
        disabledContainerColor = scheme.surfaceContainer,
        selectedContainerColor = scheme.surfaceContainer,
        selectedContentColor = scheme.onSurface,
        selectedLeadingContentColor = scheme.onSurfaceVariant,
        selectedTrailingContentColor = scheme.onSurfaceVariant,
        selectedSupportingContentColor = scheme.onSurfaceVariant,
    )
}

/**
 * The label above a group: title small in the primary color, with an
 * optional count or note at the end.
 */
@Composable
fun SectionHeader(title: String, modifier: Modifier = Modifier, trailing: String? = null) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            title,
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.weight(1f).semantics { heading() },
        )
        if (trailing != null) {
            Text(trailing, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
