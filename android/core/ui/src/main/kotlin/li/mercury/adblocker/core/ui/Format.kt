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

import android.icu.text.DateFormat
import android.icu.text.DisplayContext
import android.icu.text.RelativeDateTimeFormatter
import android.icu.text.RelativeDateTimeFormatter.AbsoluteUnit
import android.icu.text.RelativeDateTimeFormatter.Direction
import android.icu.text.RelativeDateTimeFormatter.RelativeUnit
import android.icu.util.ULocale
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import kotlinx.coroutines.delay
import java.text.NumberFormat
import java.time.Clock
import java.time.Duration
import java.time.Instant
import java.util.Locale
import kotlin.math.abs

/** A count with the locale's grouping, in tabular figures where it is shown. */
@Composable
fun formatCount(n: Long): String = NumberFormat.getIntegerInstance(LocalConfiguration.current.locales[0]).format(n)

@Composable
fun formatCount(n: Int): String = formatCount(n.toLong())

val LocalClock = staticCompositionLocalOf { Clock.systemUTC() }

@Composable
fun relativeTime(time: Instant): String = relativeTime(time, LocalClock.current.instant())

/**
 * "2 hr. ago", "yesterday", in the locale's words, relative to [now]. It is
 * capitalized for the middle of a sentence ("updated yesterday"), where the
 * app shows it; the framework's DateUtils capitalizes it for the start.
 */
fun relativeTime(time: Instant, now: Instant, locale: Locale = Locale.getDefault()): String {
    val formatter = RelativeDateTimeFormatter.getInstance(
        ULocale.forLocale(locale),
        null,
        RelativeDateTimeFormatter.Style.SHORT,
        DisplayContext.CAPITALIZATION_FOR_MIDDLE_OF_SENTENCE,
    )
    val seconds = Duration.between(time, now).seconds
    val direction = if (seconds >= 0) Direction.LAST else Direction.NEXT
    val elapsed = abs(seconds)
    val days = elapsed / SECONDS_PER_DAY
    val text = when {
        elapsed < SECONDS_PER_MINUTE -> formatter.format(Direction.PLAIN, AbsoluteUnit.NOW)
        elapsed < SECONDS_PER_HOUR -> formatter.format((elapsed / SECONDS_PER_MINUTE).toDouble(), direction, RelativeUnit.MINUTES)
        days < 1 -> formatter.format((elapsed / SECONDS_PER_HOUR).toDouble(), direction, RelativeUnit.HOURS)
        days == 1L -> formatter.format(direction, AbsoluteUnit.DAY)
        days < DAYS_PER_WEEK * 2 -> formatter.format(days.toDouble(), direction, RelativeUnit.DAYS)
        else -> DateFormat.getDateInstance(DateFormat.MEDIUM, locale).format(java.util.Date.from(time))
    }
    return if (locale.language == "zh") spacedHan(text) else text
}

/**
 * A space between digits and Chinese characters, as the app's Chinese copy
 * writes "2 小时前"; ICU writes "2小时前".
 */
private fun spacedHan(text: String): String = text
    .replace(Regex("(\\d)(\\p{IsHan})"), "$1 $2")
    .replace(Regex("(\\p{IsHan})(\\d)"), "$1 $2")

/**
 * The time left until [time] from [now], rounded up to the minute: "15
 * minutes", "1 hour", "1 hour 20 minutes".
 */
@Composable
fun timeLeft(time: Instant, now: Instant): String {
    val minutes = ((Duration.between(now, time).seconds + SECONDS_PER_MINUTE - 1) / SECONDS_PER_MINUTE).coerceAtLeast(1)
    val hours = (minutes / MINUTES_PER_HOUR).toInt()
    val rest = (minutes % MINUTES_PER_HOUR).toInt()
    return when {
        hours == 0 -> pluralStringResource(R.plurals.core_ui_minutes, rest, rest)

        rest == 0 -> pluralStringResource(R.plurals.core_ui_hours, hours, hours)

        else -> stringResource(
            R.string.core_ui_hours_minutes,
            pluralStringResource(R.plurals.core_ui_hours, hours, hours),
            pluralStringResource(R.plurals.core_ui_minutes, rest, rest),
        )
    }
}

/** How long since [time], in its largest whole unit: "12 minutes", "5 hours", "3 days". */
@Composable
fun timeSince(time: Instant, now: Instant): String {
    val seconds = Duration.between(time, now).seconds.coerceAtLeast(0)
    val minutes = (seconds / SECONDS_PER_MINUTE).toInt().coerceAtLeast(1)
    val hours = (seconds / SECONDS_PER_HOUR).toInt()
    val days = (seconds / SECONDS_PER_DAY).toInt()
    return when {
        days > 0 -> pluralStringResource(R.plurals.core_ui_days, days, days)
        hours > 0 -> pluralStringResource(R.plurals.core_ui_hours, hours, hours)
        else -> pluralStringResource(R.plurals.core_ui_minutes, minutes, minutes)
    }
}

/** The current time, read again every [periodMillis] while shown, for what counts down. */
@Composable
fun rememberNow(periodMillis: Long = NOW_PERIOD_MS): Instant {
    val clock = LocalClock.current
    var now by remember(clock) { mutableStateOf(clock.instant()) }
    LaunchedEffect(clock, periodMillis) {
        while (true) {
            delay(periodMillis)
            now = clock.instant()
        }
    }
    return now
}

private const val NOW_PERIOD_MS = 15_000L
private const val MINUTES_PER_HOUR = 60L

private const val SECONDS_PER_MINUTE = 60L
private const val SECONDS_PER_HOUR = 3_600L
private const val SECONDS_PER_DAY = 86_400L
private const val DAYS_PER_WEEK = 7L
