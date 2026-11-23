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

import androidx.annotation.StringRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.designsystem.motion.swapTransform

/**
 * Where a flow of steps is (onboarding, the HTTPS setup): one segment a
 * step, those done and the current one in primary. [current] counts from 0;
 * [label] says it in words, such as "Step 2 of 4".
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun StepProgress(current: Int, count: Int, label: String, modifier: Modifier = Modifier) {
    Row(
        modifier.semantics {
            contentDescription = label
            progressBarRangeInfo = ProgressBarRangeInfo((current + 1).toFloat(), 1f..count.toFloat(), steps = count - 2)
        },
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        repeat(count) { index ->
            val color by animateColorAsState(
                if (index <= current) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.secondaryContainer,
                MaterialTheme.motionScheme.defaultEffectsSpec(),
                label = "segment",
            )
            Box(Modifier.weight(1f).height(4.dp).background(color, RoundedCornerShape(2.dp)))
        }
    }
}

/**
 * A step of a flow: art, a title, what it is about, its content, and its
 * actions at the bottom. Lists span the page margin; words are indented a
 * little more.
 */
@Composable
fun ColumnScope.StepPage(
    title: String,
    text: String?,
    art: (@Composable () -> Unit)?,
    action: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    centered: Boolean = false,
    content: @Composable ColumnScope.() -> Unit = {},
) {
    Column(modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
        if (art != null) {
            Box(Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 4.dp), contentAlignment = Alignment.Center) { art() }
        }
        StepTitle(title, centered)
        // What the step says changes with it (setup finishing or failing): the words fade through.
        FadeThrough(text, { it }, Modifier.fillMaxWidth(), label = "text") { shown ->
            if (shown != null) {
                Text(
                    shown,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = if (centered) TextAlign.Center else TextAlign.Start,
                    modifier = Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 8.dp, bottom = 20.dp),
                )
            }
        }
        content()
        Spacer(Modifier.height(16.dp))
    }
    Box(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 16.dp)) { action() }
}

/** A step's title; a change of it (setup finishing) is swapped: the old fades, the new rises into place. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun StepTitle(title: String, centered: Boolean) {
    val motion = MaterialTheme.motionScheme
    val rise = with(LocalDensity.current) { 6.dp.roundToPx() }
    AnimatedContent(targetState = title, transitionSpec = { swapTransform(motion, rise) }, label = "title") { shown ->
        Text(
            shown,
            style = if (centered) MaterialTheme.typography.headlineLargeEmphasized else MaterialTheme.typography.headlineMediumEmphasized,
            textAlign = if (centered) TextAlign.Center else TextAlign.Start,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp).padding(top = 8.dp),
        )
    }
}

/** The one action of a step: a medium filled button across the page. */
@Composable
fun StepAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.fillMaxWidth().height(ButtonDefaults.MediumContainerHeight),
        contentPadding = ButtonDefaults.contentPaddingFor(ButtonDefaults.MediumContainerHeight),
    ) { Text(text, style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight)) }
}

/** A second action as weighty as the first, below it: restarting now or later. */
@Composable
fun StepOutlinedAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier) {
    OutlinedButton(
        onClick = onClick,
        modifier = modifier.fillMaxWidth().height(ButtonDefaults.MediumContainerHeight),
        contentPadding = ButtonDefaults.contentPaddingFor(ButtonDefaults.MediumContainerHeight),
    ) { Text(text, style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight)) }
}

/**
 * A step's main action with a lesser one above it, such as going back a
 * step, leaving for now, or checking again: a text button, which does not
 * compete with the filled one.
 */
@Composable
fun StepActions(other: String, onOther: () -> Unit, main: String, onMain: () -> Unit, modifier: Modifier = Modifier, mainEnabled: Boolean = true) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        TextButton(onClick = onOther, modifier = Modifier.fillMaxWidth()) { Text(other) }
        StepAction(main, onMain, enabled = mainEnabled)
    }
}

/** Statements, one per row: they wrap, unlike a headline. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun FactList(vararg facts: Pair<ImageVector, Int>, modifier: Modifier = Modifier) {
    SegmentedGroup(facts.size, modifier) { index ->
        val (icon, text) = facts[index]
        Fact(icon, text, index, facts.size)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Fact(icon: ImageVector, @StringRes text: Int, index: Int, count: Int) {
    SegmentedListItem(
        shapes = ListItemDefaults.segmentedShapes(index, count),
        colors = segmentedColors(),
        leadingContent = { Icon(icon, contentDescription = null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) },
    ) { Text(stringResource(text), style = MaterialTheme.typography.bodyLarge) }
}
