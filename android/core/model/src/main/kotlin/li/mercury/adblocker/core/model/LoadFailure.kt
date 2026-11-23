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

package li.mercury.adblocker.core.model

/** Why something could not be read or done, as the screens tell the user. */
enum class LoadFailure {
    /** The app has not been granted root. */
    NoRoot,

    /** The module is not installed. */
    ModuleMissing,

    /** This build cannot filter without root. */
    EngineMissing,

    /** A command took longer than it may, and was stopped. */
    TimedOut,

    /** A command failed for another reason. */
    Failed,
}
