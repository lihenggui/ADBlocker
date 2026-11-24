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

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.installCommand
import li.mercury.adblocker.core.data.toLoadFailure
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.model.LoadFailure
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.SequenceInputStream
import java.nio.file.Files
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue

class RootShellTest {
    @Test
    fun aRejectedCommandIsNotTakenForAnOlderModule() {
        for (said in listOf("usage", "", "abd: not a host: \"unknown command\"", "abd: pause takes --minutes <n> or nothing")) {
            val failure = exitFailure(2, said)
            assertIs<AbdException.CommandRejected>(failure)
            assertEquals(LoadFailure.Failed, failure.toLoadFailure(), said)
        }
    }

    @Test
    fun givenACommandAbdDoesNotKnow_whenItIsRejected_thenTheModuleIsOlderThanTheApp() {
        val failure = exitFailure(2, "abd: unknown command: app eligibility --json\n\nusage: abd <command>, as root\n")
        // Still a rejection to those that handle one.
        assertIs<AbdException.CommandRejected>(failure)
        assertIs<AbdException.ModuleOutdated>(failure)
        assertEquals(LoadFailure.ModuleOutdated, failure.toLoadFailure())
        // Only a usage error says so.
        assertIs<AbdException.CommandFailed>(exitFailure(1, "abd: unknown command: x"))
    }

    @Test
    fun aCommandStoppedByTimeoutTimedOut() {
        assertIs<AbdException.TimedOut>(exitFailure(124, ""))
        assertEquals(LoadFailure.TimedOut, exitFailure(124, "").toLoadFailure())
    }

    @Test
    fun abdNotFoundOrNotRunnableMeansTheModuleIsMissing() {
        assertIs<AbdException.ModuleMissing>(exitFailure(126, ""))
        assertIs<AbdException.ModuleMissing>(exitFailure(127, "sh: abd: not found"))
        assertEquals(LoadFailure.ModuleMissing, exitFailure(127, "").toLoadFailure())
    }

    @Test
    fun anyOtherExitIsAFailedCommandSayingWhatItPrinted() {
        val failure = exitFailure(1, " not a filtered app \n")
        assertIs<AbdException.CommandFailed>(failure)
        assertEquals("not a filtered app", failure.message)
        assertEquals("exit status 3", exitFailure(3, "").message)
        assertEquals(LoadFailure.Failed, failure.toLoadFailure())
    }

    @Test
    fun aQuotedValueIsOneWordWhateverItHolds() {
        for (value in listOf("plain", "with space", "it's", "'", "''", "\$HOME", "`id`", "a;b|c&d", "line\nbreak", "\\", "")) {
            assertEquals(value, sh("printf %s ${value.shellQuoted()}").output, value)
        }
    }

    @Test
    fun aBoundedCommandKeepsItsExitStatus() {
        assertEquals(3, sh(bounded("exit 3", 5)).code)
        assertEquals("it's\n", sh(bounded("echo ${"it's".shellQuoted()}", 5)).output)
    }

    @Test
    fun aBoundedCommandThatRunsTooLongIsStoppedWithTheTimeoutStatus() {
        assertEquals(TIMED_OUT, sh(bounded("sleep 10", 1)).code)
    }

    @Test
    fun aBoundedCommandReadsNothingFromTheSharedShell() {
        // The root shell reads its next commands from standard input.
        val ran = sh("echo next | ${bounded("cat", 5)}")
        assertEquals("", ran.output)
    }

    @Test
    fun rulesTravelThroughStdinWithoutShellExpansionOrAnExtraNewline() = runTest {
        val rules = "! '\$HOME `id` Запись\n".repeat(64 * 1024) + "||example.com^"
        val result = communicate(ProcessBuilder("sh", "-c", "cat; printf warning >&2").start(), rules, Dispatchers.IO)
        assertEquals(0, result.code)
        assertEquals(rules, result.output)
        assertEquals("warning", result.error)
    }

    @Test
    fun theRootTransportKeepsEvenASizeLimitPayloadOutOfTheCommandLine() = runTest {
        val payload = "!".repeat(16 shl 20)
        val shell = object : RootShell(Dispatchers.IO) {
            override suspend fun runWithInput(command: String, label: String, input: String): CommandResult {
                assertEquals("${ModulePaths.ABD} 'rules' 'check' '--json'", command)
                assertEquals("abd rules check", label)
                assertEquals(payload, input)
                return CommandResult(true, 0, "checked", "")
            }
        }
        assertEquals("checked", RootTransport(shell).run(listOf("rules", "check", "--json"), "rules check", payload))
    }

    @Test
    fun readingRulesPreservesAllTrailingNewlines() = runTest {
        val rules = "! comment\n\n"
        val shell = object : RootShell(Dispatchers.IO) {
            override suspend fun runWithInput(command: String, label: String, input: String): CommandResult {
                assertEquals("${ModulePaths.ABD} 'rules' 'get'", command)
                assertEquals("", input)
                return CommandResult(true, 0, rules, "")
            }
        }
        assertEquals(rules, RootTransport(shell).run(listOf("rules", "get"), "rules get"))
    }

    @Test
    fun anEarlyExitKeepsItsStatusWhenTheInputPipeCloses() = runTest {
        val result = communicate(ProcessBuilder("sh", "-c", "exit 127").start(), "x".repeat(1024 * 1024), Dispatchers.IO)
        assertEquals(127, result.code)
    }

    @Test
    fun givenAStreamReadFailure_whenCollectingThroughRootTransport_thenItIsReportedWithoutRawPipeDetails() = runTest {
        val failure = IOException("private pipe detail")
        val shell = object : RootShell(Dispatchers.IO) {
            override fun stream(command: String, label: String) = flow<String> { throw failure }
        }

        val error = assertFailsWith<AbdException.BadOutput> {
            RootTransport(shell).stream(listOf("update", "--json"), "update", endsWithInput = false).toList()
        }
        assertEquals("Could not read abd update output", error.message)
    }

    @Test
    fun givenStdoutFailsAfterOneLine_whenTheChildExitsCleanly_thenTheLineAndReadFailureAreDelivered() = runTest {
        val lines = mutableListOf<String>()
        val failure = IOException("failed stdout")
        val process = StreamProcess(stdoutFailure(failure), code = 0)

        val error = assertFailsWith<IOException> { streamProcess("fixture") { process }.toList(lines) }

        assertEquals(listOf("""{"event":"start","lists":1}"""), lines)
        assertEquals(failure.message, error.message)
        assertTrue(process.destroyed)
    }

    @Test
    fun givenStdoutFailsAndTheChildRejectsTheCommand_whenCollecting_thenItsExitStatusIsKept() = runTest {
        val process = StreamProcess(stdoutFailure(IOException("failed stdout")), code = 2)

        assertFailsWith<AbdException.CommandRejected> { streamProcess("fixture") { process }.toList() }
        assertTrue(process.destroyed)
    }

    @Test
    fun givenStdoutFailsWhileTheChildKeepsRunning_whenCollecting_thenTheWaitIsBoundedAndTheProcessIsDestroyed() = runTest {
        val failure = IOException("failed stdout")
        val process = StreamProcess(stdoutFailure(failure), code = null)

        val error = assertFailsWith<IOException> { streamProcess("fixture") { process }.toList() }

        assertEquals(failure.message, error.message)
        assertTrue(process.destroyed)
    }

    @Test
    fun givenACollectorLeavesEarly_whenItHasReadOneLine_thenItsChildProcessStops() = runTest {
        val process = ProcessBuilder("sh", "-c", "echo ready; read -r keep").start()
        try {
            assertEquals("ready", streamProcess("fixture") { process }.first())
            assertTrue(process.waitFor(5, TimeUnit.SECONDS))
        } finally {
            process.destroyForcibly()
        }
    }

    private fun stdoutFailure(failure: IOException): InputStream = SequenceInputStream(
        ByteArrayInputStream(("""{"event":"start","lists":1}""" + "\n").toByteArray()),
        object : InputStream() {
            override fun read(): Int = throw failure
        },
    )

    private class StreamProcess(private val stdout: InputStream, private val code: Int?) : Process() {
        private val stdin = ByteArrayOutputStream()
        var destroyed = false
            private set

        override fun getInputStream() = stdout
        override fun getOutputStream() = stdin
        override fun getErrorStream() = ByteArrayInputStream(byteArrayOf())
        override fun waitFor() = checkNotNull(code) { "A broken stream must not wait indefinitely" }
        override fun waitFor(timeout: Long, unit: TimeUnit): Boolean {
            assertTrue(timeout > 0)
            return code != null
        }

        override fun exitValue() = waitFor()
        override fun destroy() {
            destroyed = true
        }
    }

    @Test
    fun theInstallerIsTakenOutOfTheApkAndGivenItsPathAsOneWord() {
        val bin = fakeBin()
        val stage = bin.resolve("stage")
        // Root's directory is not the host's to write in.
        bin.script("mktemp", "mkdir ${stage.path} && echo ${stage.path}")
        // The installer in this archive prints what it was asked for.
        bin.script("unzip", "printf '%s\\n' \"\$@\" >${bin.path}/unzipped\necho 'printf \"%s\\n\" \"\$@\"'")
        val apk = "/data/app/~~x y/li.mercury.adblocker-'1'/base.apk"
        val ran = sh(installCommand(apk), bin)
        assertEquals(0, ran.code)
        assertEquals(listOf("install", apk), ran.output.lines().filter { it.isNotEmpty() })
        assertEquals(listOf("-p", apk, "direct.sh"), bin.resolve("unzipped").readLines())
        assertTrue(!stage.exists())
    }

    @Test
    fun anInstallerThatFailsKeepsItsStatusAndLeavesNothingBehind() {
        val bin = fakeBin()
        val stage = bin.resolve("stage")
        bin.script("mktemp", "mkdir ${stage.path} && echo ${stage.path}")
        bin.script("unzip", "echo 'echo \"! Unsupported architecture: riscv64\"; exit 1'")
        val ran = sh(installCommand("/base.apk"), bin)
        assertEquals(1, ran.code)
        assertEquals("! Unsupported architecture: riscv64", ran.output.trim())
        assertTrue(!stage.exists())
    }

    @Test
    fun abdIsTheManagersModuleOnceItHasItsDaemonAndElseTheOneTheAppInstalled() {
        val adb = Files.createTempDirectory("adb").toFile().apply { deleteOnExit() }
        val abd = ModulePaths.ABD.replace("/data/adb", adb.path)
        fun install(dir: String) = adb.resolve("$dir/bin").apply { mkdirs() }.script("abd", "echo $dir\nprintf '%s\\n' \"\$@\"\ncat\nexit 3")
        assertEquals(127, sh("$abd status").code)

        install("adblocker_direct")
        // What a manager keeps for a module it applies at the next boot.
        adb.resolve("modules/adblocker").mkdirs()
        val arguments = listOf("it's", "two words", "\$HOME")
        val ran = sh("echo input | $abd ${arguments.joinToString(" ") { it.shellQuoted() }}")
        assertEquals(listOf("adblocker_direct") + arguments + "input", ran.output.lines().filter { it.isNotEmpty() })
        assertEquals(3, ran.code)

        install("modules/adblocker")
        fun said(command: String) = sh(command).output.lines().filter { it.isNotEmpty() }
        assertEquals(listOf("modules/adblocker"), said("$abd </dev/null"))
        // In the forms the root shell runs it in.
        assertEquals(listOf("modules/adblocker", "status"), said(bounded("$abd status", 5)))
        assertEquals(listOf("modules/adblocker", "input"), said("echo input | exec timeout 5 $abd"))
    }
}
