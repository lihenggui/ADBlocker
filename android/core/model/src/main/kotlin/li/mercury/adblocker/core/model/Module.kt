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

/** Where the app stands with root and the module. */
enum class ModuleState {
    /** Root has not been granted to the app. */
    NoRoot,

    /**
     * No module is in use. One flashed in a root manager may wait for a
     * restart to be applied; until then the app can set up its own, which
     * the manager's replaces at that restart.
     */
    NotInstalled,

    /** The module or the bundled package could not be verified. */
    Unavailable,

    /**
     * A module is in use that is not this app's, and it cannot be replaced
     * before a restart: a root manager has an update waiting for it, or has
     * it marked for removal. Nothing can be installed until then.
     */
    NeedsRestart,

    /** The daemon in use differs from the one bundled in this app, and can be replaced with it. */
    Mismatched,

    /** The bundled daemon is installed. */
    Ready,
}

/** One check of whether the device can run the module. */
data class DeviceCheck(
    val kind: DeviceCheckKind,
    val passed: Boolean,
    /** What was found, to show beside the check (a version, a manager name). */
    val detail: String?,
)

enum class DeviceCheckKind { Root, AndroidVersion, Kernel, Architecture }
