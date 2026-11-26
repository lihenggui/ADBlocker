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

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.ui.AdaptiveSheet
import li.mercury.adblocker.core.ui.RevealNotNull

/**
 * A sheet that asks for one line, such as a list's address: what it does,
 * a filled text field, an optional [preview] of what will be written, and
 * Cancel beside the action at the end.
 */
@Composable
internal fun TextInputSheet(
    title: String,
    text: String,
    label: String,
    confirm: String,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
    preview: @Composable (String) -> String? = { null },
) {
    var value by rememberSaveable { mutableStateOf("") }
    AdaptiveSheet(onDismiss = onDismiss, title = title) {
        Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextField(
                value = value,
                onValueChange = { value = it },
                label = { Text(label) },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace, textDirection = TextDirection.Ltr, textAlign = TextAlign.Left),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
                modifier = Modifier.fillMaxWidth(),
            )
            RevealNotNull(preview(value)) {
                Text(it, style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.feature_filters_cancel)) }
                Button(
                    onClick = {
                        onConfirm(value)
                        onDismiss()
                    },
                    enabled = value.isNotBlank(),
                ) { Text(confirm) }
            }
        }
    }
}

/** The kinds of request a URL can be tested as, as abd names them. */
private val KINDS = listOf("script", "image", "document", "xmlhttprequest", "other")

/**
 * What the lists say about one request: which rule, from which list. The
 * address, the kind of request, and the page it was loaded on, as filled
 * fields; then the verdict.
 */
@Composable
internal fun TestUrlSheet(result: RuleCheckResult?, onCheck: (String, String, String) -> Unit, onDismiss: () -> Unit) {
    var url by rememberSaveable { mutableStateOf("") }
    var page by rememberSaveable { mutableStateOf("") }
    var kind by rememberSaveable { mutableStateOf("script") }
    val mono = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace, textDirection = TextDirection.Ltr, textAlign = TextAlign.Left)
    AdaptiveSheet(onDismiss = onDismiss, title = stringResource(R.string.feature_filters_test_url)) {
        Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.feature_filters_test_url_text), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextField(
                value = url,
                onValueChange = { url = it },
                label = { Text(stringResource(R.string.feature_filters_url_label)) },
                singleLine = true,
                textStyle = mono,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            Row(Modifier.horizontalScroll(rememberScrollState()).selectableGroup(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                KINDS.forEach { k ->
                    FilterChip(
                        selected = kind == k,
                        onClick = { kind = k },
                        label = { Text(kindLabel(k)) },
                        leadingIcon = if (kind == k) {
                            { Icon(AdbIcons.Check, contentDescription = null, Modifier.size(FilterChipDefaults.IconSize)) }
                        } else {
                            null
                        },
                        // Outline, not outline variant: 3:1 against the sheet (chip accessibility).
                        border = FilterChipDefaults.filterChipBorder(enabled = true, selected = kind == k, borderColor = MaterialTheme.colorScheme.outline),
                        modifier = Modifier.semantics { role = Role.RadioButton },
                    )
                }
            }
            TextField(
                value = page,
                onValueChange = { page = it },
                label = { Text(stringResource(R.string.feature_filters_page_label)) },
                singleLine = true,
                textStyle = mono,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            Button(onClick = { onCheck(url, page, kind) }, enabled = url.isNotBlank(), modifier = Modifier.align(Alignment.End)) {
                Text(stringResource(R.string.feature_filters_check))
            }
            RevealNotNull(result) { CheckResult(it) }
        }
    }
}

@Composable
private fun CheckResult(result: RuleCheckResult) {
    // Checking another address changes the verdict in place: its colors follow.
    val spec = MaterialTheme.motionScheme.defaultEffectsSpec<Color>()
    val container by animateColorAsState(
        if (result.blocked) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
        spec,
        label = "verdict",
    )
    val content by animateColorAsState(if (result.blocked) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface, spec, label = "verdict content")
    Card(
        colors = CardDefaults.cardColors(containerColor = container, contentColor = content),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(if (result.blocked) AdbIcons.Block else AdbIcons.Check, contentDescription = null)
            Column {
                Text(
                    stringResource(if (result.blocked) R.string.feature_filters_would_block else R.string.feature_filters_would_allow),
                    style = MaterialTheme.typography.titleMedium,
                )
                // The rule, and the list it is in by name: "||adnet.example^ · EasyList".
                val source = result.source?.let { sourceName(it) }
                val rule = result.rule
                if (rule != null || source != null) {
                    Text(
                        buildAnnotatedString {
                            rule?.let { withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append(it) } }
                            if (rule != null && source != null) append(" · ")
                            source?.let { append(it) }
                        },
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }
    }
}

/** The list a rule is in, by the name it is known by, or the host it comes from. */
@Composable
internal fun sourceName(source: RuleSource): String = when (source) {
    RuleSource.UserRules -> stringResource(R.string.feature_filters_source_mine)
    is RuleSource.List -> FilterCatalog.byUrl(source.url)?.title ?: source.url.substringAfter("://").substringBefore('/')
}

@Composable
private fun kindLabel(kind: String) = stringResource(
    when (kind) {
        "script" -> R.string.feature_filters_kind_script
        "image" -> R.string.feature_filters_kind_image
        "document" -> R.string.feature_filters_kind_document
        "xmlhttprequest" -> R.string.feature_filters_kind_xhr
        else -> R.string.feature_filters_kind_other
    },
)
