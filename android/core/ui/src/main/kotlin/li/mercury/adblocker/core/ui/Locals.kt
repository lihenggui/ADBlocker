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

import androidx.compose.runtime.compositionLocalOf

/**
 * Whether a list's detail is shown in a pane beside it (two-pane layouts,
 * from the expanded breakpoint). Then the list marks the selected item, and
 * the detail has no back button (M3 list-detail).
 */
val LocalShowsDetailBesideList = compositionLocalOf { false }

/** Whether the window is compact, under 600dp wide: 16dp margins rather than 24dp. */
val LocalCompactWindow = compositionLocalOf { true }

/**
 * Whether the window is expanded or wider, 840dp and up: bottom sheets give
 * way to dialogs and side panes there (M3 bottom sheets, breakpoints).
 */
val LocalWideWindow = compositionLocalOf { false }
