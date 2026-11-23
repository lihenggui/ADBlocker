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

package li.mercury.adblocker.feature.filters.api.navigation

import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

/** The filter lists, the user's own rules and the trusted sites. */
@Serializable
data object FiltersNavKey : NavKey

/** The editor of the user's own rules. */
@Serializable
data object RuleEditorNavKey : NavKey
