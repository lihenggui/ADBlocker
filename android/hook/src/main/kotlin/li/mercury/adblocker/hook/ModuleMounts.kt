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

package li.mercury.adblocker.hook

/**
 * Whether the root module mounted its copies of the stores into this
 * process, as `/proc/self/mountinfo` describes the process's mounts.
 *
 * The module bind-mounts directories of its data directory,
 * `/data/adb/adblocker`, which only root can write to, over the stores
 * (`abd/src/trust.rs`). In the mount table such a mount has, as its root,
 * the directory's path within the file system mounted at `/data`, and that
 * file system's device. A certificate file in a store says nothing of who
 * put it there: other modules copy certificates into the system store of
 * every app.
 */
internal object ModuleMounts {
    // `Paths::store` and `Paths::user_store` of the module, under `/data`;
    // a test in abd/src/trust.rs keeps them so.
    const val SYSTEM_COPY = "/adb/adblocker/store"
    const val USER_COPY = "/adb/adblocker/user-store"
    private const val DATA = "/data"

    private class Mount(val device: String, val root: String, val point: String)

    /**
     * Whether what this process sees at [point] is the module's [copy]: the
     * last mount there is that directory of `/data`'s file system, and
     * nothing is mounted over it, or over a directory above it, after it.
     */
    fun isModuleCopy(mountInfo: String, point: String, copy: String): Boolean {
        val mounts = mountInfo.lineSequence().mapNotNull(::parse).toList()
        val data = mounts.lastOrNull { it.point == DATA } ?: return false
        // Of mounts at one point, the last lies over those before it.
        val top = mounts.indexOfLast { it.point == point }
        if (top < 0) return false
        val mount = mounts[top]
        val copyRoot = data.root.trimEnd('/') + copy
        if (mount.root != copyRoot || mount.device != data.device) return false
        return mounts.drop(top + 1).none { point == it.point || point.startsWith(it.point.trimEnd('/') + "/") }
    }

    // id parent major:minor root mount-point options [optional...] - type source ...
    private fun parse(line: String): Mount? {
        val fields = line.split(' ')
        if (fields.size < 5 || "-" !in fields) return null
        return Mount(fields[2], unescape(fields[3]), unescape(fields[4]))
    }

    // The kernel writes a space, tab, newline or backslash in a path as an
    // octal escape.
    private val escape = Regex("""\\([0-7]{3})""")

    private fun unescape(path: String): String = escape.replace(path) { it.groupValues[1].toInt(8).toChar().toString() }
}
