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

package li.mercury.adblocker.core.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.ModulePaths.ABD
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.fakeBin
import li.mercury.adblocker.core.data.root.script
import li.mercury.adblocker.core.data.root.sh
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.data.transport.endingWithInput
import li.mercury.adblocker.core.model.FilterLevel
import java.io.File
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class AddAppTest {
    private val bin = fakeBin()
    private val calls: File = bin.resolve("calls").apply { deleteOnExit() }

    /**
     * An abd that writes down each command it is given, one a line, and
     * exits with what [answer] says for it: a shell `case` on "$*".
     */
    private fun abd(answer: String) = bin.script("abd", "echo \"\$*\" >>${calls.absolutePath}\ncase \"\$*\" in\n$answer\nesac\nexit 0")

    /** A root shell that runs each command with the host's sh, the fake abd in place of the module's. */
    private fun client(abd: File): AbdClient {
        val root = RootTransport(
            object : RootShell(Dispatchers.Unconfined) {
                override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
                    val ran = sh(command.replace(ABD, abd.absolutePath))
                    return CommandResult(ran.code == 0, ran.code, ran.output, "")
                }
            },
        )
        return DefaultAbdClient(
            object : TransportSource {
                override suspend fun current() = root
            },
        )
    }

    private fun calls() = calls.readLines()

    @Test
    fun anAppWithoutAModeIsAddedAsItWas() = runTest {
        client(abd("")).addApp("com.example.news")
        assertEquals(listOf("app add com.example.news"), calls())
    }

    @Test
    fun aRejectedAddDoesNotRetryWithOtherModes() = runTest {
        val abd = abd(""" "app add com.example.bank") exit 2 ;; """)
        assertFailsWith<AbdException.CommandRejected> { client(abd).addApp("com.example.bank") }
        assertEquals(listOf("app add com.example.bank"), calls())
    }

    @Test
    fun anUpdateEndsWhenItsInputDoes() {
        // exec: the process signalled is the one that would go on.
        val abd = bin.script("slow", "echo ready\nexec sleep 30")
        val process = ProcessBuilder("sh", "-c", endingWithInput(abd.absolutePath)).start()
        try {
            assertEquals("ready", process.inputStream.bufferedReader().readLine())
            process.outputStream.close()
            assertEquals(true, process.waitFor(5, TimeUnit.SECONDS), "still running after EOF")
            assertEquals(143, process.exitValue())
        } finally {
            process.destroyForcibly()
        }
    }

    @Test
    fun anUpdateThatEndsByItselfKeepsItsStatus() {
        val abd = bin.script("quick", "echo done\nexit 3")
        val process = ProcessBuilder("sh", "-c", endingWithInput(abd.absolutePath)).start()
        try {
            // RootShell.stream keeps this pipe open until its collector leaves.
            assertEquals(true, process.waitFor(5, TimeUnit.SECONDS))
            assertEquals(3, process.exitValue())
            assertEquals("done", process.inputStream.bufferedReader().readLine())
        } finally {
            process.outputStream.close()
            process.destroyForcibly()
        }
    }
}
