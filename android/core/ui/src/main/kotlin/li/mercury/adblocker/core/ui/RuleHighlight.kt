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

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle

/** The colors of the parts of a filter rule. */
data class RuleColors(
    val comment: Color,
    val anchor: Color,
    val option: Color,
    val exception: Color,
    val cosmetic: Color,
)

/**
 * Filter rules in the syntax of the lists (EasyList, uBlock Origin,
 * AdGuard), with their parts colored: comments, the `||` and `^` anchors and
 * separators, `$` options, `@@` exceptions, and element hiding, which the
 * daemon does not apply, underlined. Only colors change: the text, and so
 * every offset in it, stays as it is.
 */
fun highlightRules(text: String, colors: RuleColors): AnnotatedString = buildAnnotatedString {
    text.split('\n').forEachIndexed { index, line ->
        if (index > 0) append('\n')
        highlightLine(line, colors)
    }
}

private fun AnnotatedString.Builder.highlightLine(line: String, colors: RuleColors) {
    when {
        line.startsWith('!') || line.startsWith('[') -> withStyle(SpanStyle(color = colors.comment, fontStyle = FontStyle.Italic)) { append(line) }

        line.contains("##") || line.contains("#@#") || line.contains("#?#") || line.contains("#$#") || line.contains("#%#") -> {
            val at = listOf("##", "#@#", "#?#", "#$#", "#%#").mapNotNull { m -> line.indexOf(m).takeIf { it >= 0 } }.min()
            append(line.substring(0, at))
            withStyle(SpanStyle(color = colors.cosmetic, textDecoration = TextDecoration.Underline)) { append(line.substring(at)) }
        }

        else -> {
            var rest = line
            if (rest.startsWith("@@")) {
                withStyle(SpanStyle(color = colors.exception, fontWeight = FontWeight.Bold)) { append("@@") }
                rest = rest.substring(2)
            }
            val dollar = rest.lastIndexOf('$').takeIf { it > 0 && !rest.substring(it).contains('/') } ?: -1
            val pattern = if (dollar >= 0) rest.substring(0, dollar) else rest
            pattern.forEachIndexed { i, c ->
                val anchor = (c == '|' && (i < 2 || i == pattern.lastIndex)) || c == '^' || c == '*'
                if (anchor) withStyle(SpanStyle(color = colors.anchor, fontWeight = FontWeight.Medium)) { append(c) } else append(c)
            }
            if (dollar >= 0) withStyle(SpanStyle(color = colors.option)) { append(rest.substring(dollar)) }
        }
    }
}
