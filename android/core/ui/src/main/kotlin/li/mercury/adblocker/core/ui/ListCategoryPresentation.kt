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

import li.mercury.adblocker.core.designsystem.icon.AdbIcons
import li.mercury.adblocker.core.model.ListCategory

fun categoryIcon(category: ListCategory?) = when (category) {
    ListCategory.Privacy -> AdbIcons.VisibilityOff
    ListCategory.Security -> AdbIcons.Security
    ListCategory.Regional -> AdbIcons.Language
    ListCategory.Annoyances -> AdbIcons.Tune
    ListCategory.Ads, null -> AdbIcons.Block
}

fun categoryName(category: ListCategory) = when (category) {
    ListCategory.Ads -> R.string.core_ui_category_ads
    ListCategory.Privacy -> R.string.core_ui_category_privacy
    ListCategory.Security -> R.string.core_ui_category_security
    ListCategory.Regional -> R.string.core_ui_category_regional
    ListCategory.Annoyances -> R.string.core_ui_category_annoyances
}
