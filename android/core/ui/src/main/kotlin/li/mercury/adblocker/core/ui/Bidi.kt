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

/** Keeps technical syntax left-to-right without affecting the surrounding sentence. */
fun isolateLtr(value: String): String = "\u2066$value\u2069"

/** App labels can use any script. A package name or shared UID used as a label is technical syntax. */
fun appLabelArgument(label: String, packageName: String): String = if (label == packageName.removePrefix("@")) isolateLtr(label) else label
