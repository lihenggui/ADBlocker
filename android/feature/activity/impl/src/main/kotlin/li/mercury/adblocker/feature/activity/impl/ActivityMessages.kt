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

package li.mercury.adblocker.feature.activity.impl

import dagger.hilt.android.scopes.ActivityRetainedScoped
import li.mercury.adblocker.core.ui.MessageQueue
import javax.inject.Inject

/** Request outcomes are shown by the list, even when the sheet closes or the activity rotates. */
@ActivityRetainedScoped
class ActivityMessages @Inject constructor() {
    internal val queue = MessageQueue()
}
