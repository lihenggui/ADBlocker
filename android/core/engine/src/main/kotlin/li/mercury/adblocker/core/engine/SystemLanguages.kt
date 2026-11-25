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

package li.mercury.adblocker.core.engine

import android.content.Context
import androidx.core.app.LocaleManagerCompat
import java.util.TimeZone

// Regional list defaults use only the system's locale and time zone
// settings. They need no permission or network lookup. The settings and
// their inferred regions are kept in memory and never logged or sent.

/**
 * The language tags of the device's locales, in the user's order: those the
 * engine makes the regional lists defaults for. The system's, not the
 * language chosen for this app.
 */
fun systemLanguageTags(context: Context): List<String> {
    val locales = LocaleManagerCompat.getSystemLocales(context)
    return (0 until locales.size()).mapNotNull { locales[it]?.toLanguageTag() }
}

fun systemTimeZoneId(): String = TimeZone.getDefault().id
