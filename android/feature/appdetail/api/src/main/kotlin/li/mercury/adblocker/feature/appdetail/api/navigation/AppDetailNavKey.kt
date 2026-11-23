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

package li.mercury.adblocker.feature.appdetail.api.navigation

import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable
import li.mercury.adblocker.core.navigation.Navigator

/** One app: whether and how it is filtered, what was blocked, and what to do when it misbehaves. */
@Serializable
data class AppDetailNavKey(val packageName: String) : NavKey

fun Navigator.navigateToAppDetail(packageName: String) {
    // One app at a time: picking app after app in the two-pane layout must
    // not stack their pages.
    state.currentSubStack.removeAll { it is AppDetailNavKey }
    navigate(AppDetailNavKey(packageName))
}
