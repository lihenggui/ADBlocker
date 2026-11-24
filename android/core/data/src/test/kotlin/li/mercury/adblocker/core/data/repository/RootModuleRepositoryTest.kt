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

import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ApplicationInfo
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.protocol.parseStatus
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.script
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import java.io.IOException
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The repository's commands run in the host's `sh`, with a directory of the
 * test's standing for `/data/adb`: where the module is found is the shell's
 * doing, and is what is tested.
 */
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE, sdk = [37])
class RootModuleRepositoryTest {
    @get:Rule
    val temporaryFolder = TemporaryFolder()

    private val application: Context by lazy { RuntimeEnvironment.getApplication() }
    private val bundledCode by lazy { application.packageManager.getPackageInfo(application.packageName, 0).longVersionCode }
    private val adb by lazy { temporaryFolder.newFolder("adb") }
    private val flashed by lazy { adb.resolve("modules/adblocker") }
    private val direct by lazy { adb.resolve("adblocker_direct") }
    private val pending by lazy { adb.resolve("modules_update/adblocker") }
    private val started by lazy { adb.resolve("started") }

    // Without daemons in it, the module is compared by its version alone.
    private val apk by lazy {
        temporaryFolder.newFile("base.apk").also { file ->
            ZipOutputStream(file.outputStream()).use { zip ->
                zip.putNextEntry(ZipEntry("classes.dex"))
                zip.closeEntry()
            }
        }
    }

    private var checks = 0
    private var readyAt = Int.MAX_VALUE
    private var rootGranted = true
    private var rootManagerAnswer = "Magisk"
    private var rootManagerAsked = 0
    private var installer: () -> CommandResult = { CommandResult(true, 0, "", "") }
    private val settings = TestUserDataRepository()
    private val module by lazy { repository() }

    private fun repository() = RootModuleRepository(
        context = object : ContextWrapper(application) {
            override fun getApplicationInfo() = ApplicationInfo().apply { sourceDir = apk.path }
        },
        rootShell = object : RootShell(Dispatchers.Unconfined) {
            override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
                if (!rootGranted) throw AbdException.NoRoot()
                return when (label) {
                    "root manager" -> CommandResult(true, 0, rootManagerAnswer, "").also { rootManagerAsked++ }
                    "module install" -> installer()
                    else -> onDevice(command)
                }
            }
        },
        abd = object : AbdClient by unexpected() {
            override suspend fun status() = parseStatus("""{"running":${++checks >= readyAt},"paused":false}""")
        },
        changes = DataChanges(),
        userData = settings,
        ioDispatcher = Dispatchers.Unconfined,
    )

    /** As the root shell answers: what the command printed, without what it complained of. */
    private fun onDevice(command: String): CommandResult {
        val process = ProcessBuilder("sh", "-c", "exec 2>/dev/null\n" + command.replace("/data/adb", adb.path)).start()
        val output = process.inputStream.bufferedReader().readText().trimEnd('\n')
        val code = process.waitFor()
        return CommandResult(code == 0, code, output, "")
    }

    /** A module in [dir], whose service script writes down where it ran from. */
    private fun installed(dir: File, versionCode: Long = bundledCode) = dir.apply {
        resolve("bin").mkdirs()
        resolve("bin").script("abd", "exit 0")
        resolve("module.prop").writeText("id=adblocker\nversion=v$versionCode.0\nversionCode=$versionCode\n")
        script("service.sh", "echo $name >>${started.path}")
        // What module/direct.sh does on `remove`, when it can.
        script("direct.sh", "[ \"\$1\" = remove ] && [ ! -e $path/keep ] && rm -rf $path")
    }

    private fun starts() = if (started.exists()) started.readLines() else emptyList()

    private fun rememberedAsTheApps() = settings.userData.value.moduleInstalledByApp

    @Test
    fun givenAnUnreadableModule_whenCheckingReadiness_thenTheModuleIsUnverified() = runTest {
        installed(direct).resolve("bin/abd").setExecutable(false)
        assertEquals(ModuleState.Unavailable, module.state())
    }

    @Test
    fun givenNoModule_whenCheckingReadiness_thenTheModuleIsNotInstalled() = runTest {
        assertEquals(ModuleState.NotInstalled, module.state())
    }

    @Test
    fun givenNoRoot_whenCheckingReadiness_thenRootIsReportedMissing() = runTest {
        settings.setModuleInstalledByApp(true)
        rootGranted = false
        assertEquals(ModuleState.NoRoot, module.state())
        assertFalse(module.isInstalledByApp())
        // Root not answering says nothing of whose the module is.
        assertTrue(rememberedAsTheApps())
        assertFailsWith<AbdException.NoRoot> { module.startUnasked() }
    }

    @Test
    fun givenAModuleInstalledByTheApp_whenCheckingReadiness_thenItIsReady() = runTest {
        installed(direct)
        assertEquals(ModuleState.Ready, module.state())
        assertEquals("v$bundledCode.0", module.installedVersion())
        // Remembered with the state, for the boot that follows.
        assertTrue(rememberedAsTheApps())
        assertTrue(module.isInstalledByApp())
    }

    @Test
    fun givenAModuleInTheManager_whenTheAppInstalledOneBefore_thenTheManagersIsTheOneInUse() = runTest {
        installed(direct)
        installed(flashed, versionCode = bundledCode + 1)
        assertEquals(ModuleState.Mismatched, module.state())
        assertEquals("v${bundledCode + 1}.0", module.installedVersion())
        settings.setModuleInstalledByApp(true)
        assertFalse(module.isInstalledByApp())
        assertFalse(rememberedAsTheApps())
    }

    @Test
    fun givenAModuleWaitingInTheManager_whenNoneIsInUse_thenARestartIsNeeded() = runTest {
        // What a manager keeps for a module it applies at the next boot.
        flashed.mkdirs()
        flashed.resolve("module.prop").writeText("versionCode=$bundledCode\n")
        pending.mkdirs()
        assertEquals(ModuleState.NeedsRestart, module.state())
    }

    @Test
    fun givenAModuleWaitingInTheManager_whenThisAppsModuleIsInUse_thenItIsReady() = runTest {
        installed(direct)
        flashed.mkdirs()
        pending.mkdirs()
        assertEquals(ModuleState.Ready, module.state())
        assertTrue(module.isInstalledByApp())
    }

    @Test
    fun givenAModuleWaitingInTheManager_whenAnotherVersionIsInUse_thenARestartIsNeeded() = runTest {
        installed(flashed, versionCode = bundledCode + 1)
        assertEquals(ModuleState.Mismatched, module.state())
        pending.mkdirs()
        assertEquals(ModuleState.NeedsRestart, module.state())
    }

    @Test
    fun givenSomethingElseWhereAModuleWaits_whenCheckingReadiness_thenTheModuleIsUnverified() = runTest {
        installed(direct)
        pending.parentFile!!.mkdirs()
        pending.writeText("")
        assertEquals(ModuleState.Unavailable, module.state())
    }

    @Test
    fun givenAnOlderModuleThatIsOn_whenAskedWhetherToReplaceItUnasked_thenItIsToBeReplaced() = runTest {
        installed(direct, versionCode = bundledCode - 1)
        assertTrue(module.needsUpdate())
        direct.deleteRecursively()
        installed(flashed, versionCode = bundledCode - 1)
        assertTrue(module.needsUpdate())
    }

    @Test
    fun givenAModuleThatInstallingWouldTurnOn_whenAskedWhetherToReplaceItUnasked_thenItIsLeft() = runTest {
        installed(direct, versionCode = bundledCode - 1).resolve("disable").writeText("")
        assertFalse(module.needsUpdate())
        direct.deleteRecursively()
        installed(flashed, versionCode = bundledCode - 1).resolve("remove").writeText("")
        assertFalse(module.needsUpdate())
    }

    @Test
    fun givenAModuleThatIsNotOlder_whenAskedWhetherToReplaceItUnasked_thenItIsLeft() = runTest {
        assertFalse(module.needsUpdate())
        installed(direct)
        assertFalse(module.needsUpdate())
        // What a newer daemon wrote, this app's may not read.
        installed(direct, versionCode = bundledCode + 1)
        assertEquals(ModuleState.Mismatched, module.state())
        assertFalse(module.needsUpdate())
    }

    @Test
    fun givenAModuleWaitingInTheManager_whenAskedWhetherToReplaceTheOlderOneUnasked_thenItIsLeftToTheRestart() = runTest {
        installed(flashed, versionCode = bundledCode - 1)
        pending.mkdirs()
        assertFalse(module.needsUpdate())
    }

    @Test
    fun givenAnUnreadableApk_whenVerifyingTheBundledDaemon_thenVerificationFails() {
        val apk = temporaryFolder.newFile("app.apk").apply { writeText("not a ZIP") }

        assertFailsWith<IOException> { bundledDaemonHash(apk.path, arrayOf("x86_64")) }
    }

    @Test
    fun givenAnApkWithoutDaemons_whenVerifyingTheBundledDaemon_thenTheVersionCheckCanStandAlone() {
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
    fun givenAnInstallerSucceeds_whenItsModuleIsInUse_thenItIsActiveAtOnce() = runTest {
        installer = {
            installed(direct)
            CommandResult(true, 0, "- Installing with root alone", "")
        }
        assertTrue(module.installBundled())
    }

    @Test
    fun givenAnInstallerSucceeds_whenNoModuleOfThisAppIsInUse_thenInstallationFails() = runTest {
        assertFailsWith<AbdException.CommandFailed> { module.installBundled() }
        // One waiting in the manager is not what was installed.
        pending.mkdirs()
        assertFailsWith<AbdException.CommandFailed> { module.installBundled() }
    }

    @Test
    fun givenAnInstallerFails_whenInstalling_thenWhatItPrintedIsTheFailure() = runTest {
        installer = { CommandResult(false, 1, "! Unsupported architecture: riscv64", "") }
        val failure = assertFailsWith<AbdException.CommandFailed> { module.installBundled() }
        assertEquals("! Unsupported architecture: riscv64", failure.message)
    }

    @Test
    fun givenAModuleInstalledByTheApp_whenRemoved_thenItsOwnScriptRemovesIt() = runTest {
        installed(direct)
        assertTrue(module.isInstalledByApp())
        module.remove()
        assertFalse(direct.exists())
        assertFalse(rememberedAsTheApps())
        assertEquals(ModuleState.NotInstalled, module.state())
    }

    @Test
    fun givenARemovalThatLeavesTheModule_whenRemoved_thenItFails() = runTest {
        installed(direct).resolve("keep").writeText("")
        assertTrue(module.isInstalledByApp())
        assertFailsWith<AbdException.CommandFailed> { module.remove() }
        assertTrue(rememberedAsTheApps())
    }

    @Test
    fun givenAModuleInTheManager_whenRemoved_thenNothingIsRemoved() = runTest {
        installed(direct)
        installed(flashed)
        assertFailsWith<AbdException.CommandFailed> { module.remove() }
        assertTrue(direct.exists() && flashed.exists())
    }

    @Test
    fun aServiceThatStaysStoppedTimesOut() = runTest {
        installed(direct)
        assertFailsWith<AbdException.TimedOut> { module.startDaemon() }
        assertEquals(listOf(direct.name), starts())
        assertTrue(checks > 1)
    }

    @Test
    fun aServiceThatComesUpReportsSuccess() = runTest {
        installed(flashed)
        readyAt = 3
        assertTrue(module.startDaemon())
        assertEquals(listOf(flashed.name), starts())
        assertEquals(3, checks)
    }

    @Test
    fun aDisabledModuleDoesNotStartTheService() = runTest {
        installed(flashed).resolve("disable").writeText("")
        assertFalse(module.startDaemon())
        assertEquals(emptyList(), starts())
        assertEquals(0, checks)
    }

    @Test
    fun givenAModuleMarkedForRemoval_whenTurnedOnAgain_thenTheServiceIsNotStarted() = runTest {
        installed(flashed).resolve("remove").writeText("")
        flashed.resolve("disable").writeText("")
        assertFalse(module.turnOnAgain())
        assertFalse(flashed.resolve("disable").exists())
        assertEquals(emptyList(), starts())
    }

    @Test
    fun givenAModuleInstalledByTheAppTurnedItselfOff_whenStarted_thenItIsTurnedOnAgain() = runTest {
        installed(direct).resolve("disable").writeText("")
        readyAt = 1
        assertTrue(module.startDaemon())
        assertFalse(direct.resolve("disable").exists())
        assertEquals(listOf(direct.name), starts())
    }

    @Test
    fun givenAModuleInstalledByTheAppIsStopped_whenStartedUnasked_thenItIsStarted() = runTest {
        installed(direct)
        readyAt = 2
        assertTrue(module.startUnasked())
        assertEquals(listOf(direct.name), starts())
        assertTrue(rememberedAsTheApps())
    }

    @Test
    fun givenTheDaemonRunsAlready_whenStartedUnasked_thenNothingIsStarted() = runTest {
        installed(direct)
        readyAt = 1
        assertFalse(module.startUnasked())
        assertEquals(emptyList(), starts())
    }

    @Test
    fun givenAModuleInstalledByTheAppTurnedItselfOff_whenStartedUnasked_thenItStaysOff() = runTest {
        installed(direct).resolve("disable").writeText("")
        assertFalse(module.startUnasked())
        assertTrue(direct.resolve("disable").exists())
        assertEquals(emptyList(), starts())
    }

    @Test
    fun givenAModuleInTheManager_whenStartedUnasked_thenItIsLeftToTheManager() = runTest {
        installed(direct)
        installed(flashed)
        settings.setModuleInstalledByApp(true)
        assertFalse(module.startUnasked())
        assertEquals(emptyList(), starts())
        assertEquals(0, checks)
        assertFalse(rememberedAsTheApps())
    }

    @Test
    fun givenNoModule_whenStarted_thenTheMissingScriptIsTheFailure() = runTest {
        val failure = assertFailsWith<AbdException.CommandFailed> { module.startDaemon() }
        assertEquals("service.sh not found", failure.message)
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
