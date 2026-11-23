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

package li.mercury.adblocker.core.data.repository

import android.content.ContextWrapper
import android.content.pm.ApplicationInfo
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.protocol.parseStatus
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.io.IOException
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class RootModuleRepositoryTest {
    @get:Rule
    val temporaryFolder = TemporaryFolder()

    private var disabled = false
    private var starts = 0
    private var checks = 0
    private var readyAt = Int.MAX_VALUE
    private var moduleProbe = CommandResult(false, 1, "", "")
    private var rootGranted = true
    private var pending = false
    private var rootManagerAnswer = "Magisk"
    private var rootManagerAsked = 0
    private val module = RootModuleRepository(
        context = object : ContextWrapper(null) {
            override fun getApplicationInfo() = ApplicationInfo().apply { sourceDir = "/app.apk" }
        },
        rootShell = object : RootShell(Dispatchers.Unconfined) {
            override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult = when (label) {
                "module state" -> {
                    if (!rootGranted) throw AbdException.NoRoot()
                    if (pending) CommandResult(true, 0, "restart", "") else moduleProbe
                }

                "root manager" -> CommandResult(true, 0, rootManagerAnswer, "").also { rootManagerAsked++ }

                "module install" -> CommandResult(true, 0, "", "")

                "daemon start" -> CommandResult(true, 0, "", "").also { starts++ }

                "[" -> CommandResult(disabled || pending, if (disabled || pending) 0 else 1, "", "")

                else -> error("Unexpected command")
            }
        },
        abd = object : AbdClient by unexpected() {
            override suspend fun status() = parseStatus("""{"running":${++checks >= readyAt},"paused":false}""")
        },
        changes = DataChanges(),
        ioDispatcher = Dispatchers.Unconfined,
    )

    @Test
    fun givenANonzeroModuleProbe_whenCheckingReadiness_thenTheModuleIsUnverified() = runTest {
        assertEquals(ModuleState.Unavailable, module.state())
    }

    @Test
    fun givenAnExplicitAbsentResult_whenCheckingReadiness_thenTheModuleIsNotInstalled() = runTest {
        moduleProbe = CommandResult(true, 0, "absent", "")
        assertEquals(ModuleState.NotInstalled, module.state())
    }

    @Test
    fun givenNoRoot_whenCheckingReadiness_thenRootIsReportedMissing() = runTest {
        rootGranted = false
        assertEquals(ModuleState.NoRoot, module.state())
    }

    @Test
    fun givenAStagedUpdate_whenCheckingReadiness_thenRestartTakesPrecedence() = runTest {
        pending = true
        assertEquals(ModuleState.NeedsRestart, module.state())
    }

    @Test
    fun givenAnUnreadableApk_whenVerifyingTheBundledDaemon_thenVerificationFails() {
        val apk = temporaryFolder.newFile("app.apk").apply { writeText("not a ZIP") }

        assertFailsWith<IOException> { bundledDaemonHash(apk.path, arrayOf("x86_64")) }
    }

    @Test
    fun givenAnApkWithoutDaemons_whenVerifyingTheBundledDaemon_thenTheVersionCheckCanStandAlone() {
        val apk = temporaryFolder.newFile("app.apk")
        ZipOutputStream(apk.outputStream()).use { zip ->
            zip.putNextEntry(ZipEntry("classes.dex"))
            zip.closeEntry()
        }

        assertNull(bundledDaemonHash(apk.path, arrayOf("x86_64")))
    }

    @Test
    fun givenAnApkWithOnlyTheOtherDaemon_whenVerifyingTheBundledDaemon_thenVerificationFails() {
        val apk = temporaryFolder.newFile("app.apk")
        ZipOutputStream(apk.outputStream()).use { zip ->
            zip.putNextEntry(ZipEntry("bin/arm64/abd"))
            zip.write(byteArrayOf(1))
            zip.closeEntry()
        }

        assertFailsWith<IOException> { bundledDaemonHash(apk.path, arrayOf("x86_64")) }
        assertEquals(64, bundledDaemonHash(apk.path, arrayOf("arm64-v8a"))?.length)
    }

    @Test
    fun givenAnInstallerSucceeds_whenTheNewModuleProbeFails_thenInstallationCannotClaimRestart() = runTest {
        assertFailsWith<AbdException.CommandFailed> { module.installBundled() }
    }

    @Test
    fun givenAnInstallerSucceedsAndStagesAnUpdate_whenCheckingActivation_thenInstallationRequiresRestart() = runTest {
        pending = true
        assertFalse(module.installBundled())
    }

    @Test
    fun aServiceThatStaysStoppedTimesOut() = runTest {
        assertFailsWith<AbdException.TimedOut> { module.startDaemon() }
        assertEquals(1, starts)
        assertTrue(checks > 1)
    }

    @Test
    fun aServiceThatComesUpReportsSuccess() = runTest {
        readyAt = 3
        assertTrue(module.startDaemon())
        assertEquals(3, checks)
    }

    @Test
    fun aDisabledModuleDoesNotStartTheService() = runTest {
        disabled = true
        assertFalse(module.startDaemon())
        assertEquals(0, starts)
        assertEquals(0, checks)
    }

    @Test
    fun givenARootManagerWasFound_whenAskedAgain_thenItIsNotAskedForAgain() = runTest {
        assertEquals("Magisk", module.rootManager())
        assertEquals("Magisk", module.rootManager())
        assertEquals(1, rootManagerAsked)
    }

    @Test
    fun givenNoRootManagerWasFound_whenAskedAgain_thenItIsAskedForAgain() = runTest {
        rootManagerAnswer = ""
        assertNull(module.rootManager())
        rootManagerAnswer = "KernelSU"
        assertEquals("KernelSU", module.rootManager())
        assertEquals(2, rootManagerAsked)
    }
}
