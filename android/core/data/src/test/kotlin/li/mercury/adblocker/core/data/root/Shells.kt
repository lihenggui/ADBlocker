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

package li.mercury.adblocker.core.data.root

import java.io.File
import java.nio.file.Files

/** What a shell command printed, and its exit status. */
internal class Ran(val code: Int, val output: String)

/**
 * Runs [command] with the host's own `sh`, as the root shell would on the
 * device, with [bin] first on the path: the shell's quoting and the commands'
 * exit statuses are what is tested, not abd.
 */
internal fun sh(command: String, bin: File? = null): Ran {
    val process = ProcessBuilder("sh", "-c", command).redirectErrorStream(true).apply {
        if (bin != null) environment()["PATH"] = "${bin.absolutePath}:${System.getenv("PATH")}"
    }.start()
    val output = process.inputStream.bufferedReader().readText()
    return Ran(process.waitFor(), output)
}

/** A directory for fake commands, removed when the JVM exits. */
internal fun fakeBin(): File = Files.createTempDirectory("bin").toFile().apply { deleteOnExit() }

/** Writes an executable `sh` script called [name] into this directory. */
internal fun File.script(name: String, body: String): File = resolve(name).apply {
    writeText("#!/bin/sh\n$body\n")
    setExecutable(true)
    deleteOnExit()
}
