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

package li.mercury.adblocker.feature.activity.api.navigation

import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

/** The filtered apps' requests, as they happen. */
@Serializable
data object ActivityNavKey : NavKey

/**
 * One request, opened from the Activity list, by its id. What it was is read
 * from memory (ActivityRepository.request), never carried here: the back
 * stack is saved with the activity's state, which leaves the process and
 * outlives it. After the process ended the request is gone, and its page
 * closes.
 */
@Serializable
data class RequestNavKey(val id: Long) : NavKey
