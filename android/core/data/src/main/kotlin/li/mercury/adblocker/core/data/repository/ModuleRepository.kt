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
import android.os.Build
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.root.ModulePaths.ABD
import li.mercury.adblocker.core.data.root.ModulePaths.MODULE_DIR
import li.mercury.adblocker.core.data.root.ModulePaths.MODULE_PROP
import li.mercury.adblocker.core.data.root.ModulePaths.PENDING_UPDATE_DIR
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.shellQuoted
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.model.ModuleState
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest
import java.util.zip.ZipFile
import javax.inject.Inject
import javax.inject.Singleton

/** The root module (KernelSU, Magisk or APatch) that does the filtering. */
interface ModuleRepository {
    /** Where the app stands with root and the module. */
    suspend fun state(): ModuleState

    /** The installed module's version name, or null if it is not installed or root is missing. */
    suspend fun installedVersion(): String?

    /** Which root manager is installed: "KernelSU", "Magisk", "APatch", or null. */
    suspend fun rootManager(): String?

    /**
     * Flashes the module this app's own APK carries, with the installer of the
     * root manager present, and returns whether it is active at once (KernelSU
     * applies a module without a restart; Magisk and APatch at the next boot).
     *
     * @throws AbdException if root is missing or the installer fails.
     */
    suspend fun installBundled(): Boolean

    /**
     * Starts the daemon through the module's own service script, as at boot,
     * and waits a few seconds for it to run. Returns false if the module
     * is disabled or marked for removal, and throws [AbdException.TimedOut]
     * if the service did not become ready.
     */
    suspend fun startDaemon(): Boolean

    /**
     * Turns the module on again after its boot guard turned it off, as the
     * root manager's switch would, and starts the daemon. Returns false when
     * the module is marked for removal.
     */
    suspend fun turnOnAgain(): Boolean

    /** Restarts the device, as a new module needs with Magisk and APatch. */
    suspend fun restartDevice()
}

// A singleton, so that the bundled daemon is hashed once.
@Singleton
internal class RootModuleRepository(
    private val context: Context,
    private val rootShell: RootShell,
    private val abd: AbdClient,
    private val changes: DataChanges,
    private val ioDispatcher: CoroutineDispatcher,
) : ModuleRepository {
    @Inject
    constructor(
        @ApplicationContext context: Context,
        rootShell: RootShell,
        root: RootTransport,
        changes: DataChanges,
        @Dispatcher(IO) ioDispatcher: CoroutineDispatcher,
    ) : this(
        context,
        rootShell,
        root.fixedAbdClient(),
        changes,
        ioDispatcher,
    )

    @Volatile private var bundledHash: String? = null

    override suspend fun state(): ModuleState = suspendRunCatching { verifyState() }.getOrElse {
        if (it is AbdException.NoRoot) ModuleState.NoRoot else ModuleState.Unavailable
    }

    private suspend fun verifyState(): ModuleState {
        val result = rootShell.run(
            """
            if [ -d $PENDING_UPDATE_DIR ]; then
                echo restart
            elif [ -e $PENDING_UPDATE_DIR ] || [ -L $PENDING_UPDATE_DIR ]; then
                exit 1
            elif [ ! -e $MODULE_DIR ] && [ ! -L $MODULE_DIR ]; then
                echo absent
            else
                [ -x $ABD ] && sed -n 's/^versionCode=/version /p' $MODULE_PROP
            fi
            """.trimIndent(),
            label = "module state",
        )
        if (!result.isSuccess) return ModuleState.Unavailable
        val code = when (val output = result.output.trim()) {
            "restart" -> return ModuleState.NeedsRestart
            "absent" -> return ModuleState.NotInstalled
            else -> output.removePrefix("version ").toLongOrNull() ?: return ModuleState.Unavailable
        }
        if (code != bundledVersionCode()) return ModuleState.Mismatched
        return verifyDaemon()
    }

    override suspend fun installedVersion(): String? = try {
        rootShell.run("sed -n 's/^version=//p' $MODULE_PROP").output.lineSequence().firstOrNull()?.trim()?.ifEmpty { null }
    } catch (_: AbdException.NoRoot) {
        null
    }

    override suspend fun rootManager(): String? = try {
        rootShell.run(
            "if magisk -V >/dev/null 2>&1; then echo Magisk; elif [ -x /data/adb/ksud ]; then echo KernelSU; " +
                "elif [ -x /data/adb/apd ]; then echo APatch; fi",
            label = "root manager",
        ).output.trim().ifEmpty { null }
    } catch (_: AbdException.NoRoot) {
        null
    }

    override suspend fun installBundled(): Boolean {
        val result = rootShell.run(installCommand(context.applicationInfo.sourceDir), label = "module install", timeoutSeconds = INSTALL_TIMEOUT_SECONDS)
        if (!result.isSuccess) throw AbdException.CommandFailed(result.output.ifBlank { result.error })
        return when (state()) {
            ModuleState.Ready -> true
            ModuleState.NeedsRestart -> false
            else -> throw AbdException.CommandFailed("Could not verify the installed module")
        }
    }

    override suspend fun startDaemon(): Boolean = changes.changing {
        if (disabledOrRemoved()) return@changing false
        // The script leaves the supervisor running in the background; with
        // its output detached, the shell returns once it has been started.
        val script = "$MODULE_DIR/service.sh"
        val result = rootShell.run(
            "if [ -f $script ]; then sh $script </dev/null >/dev/null 2>&1; else echo '$script not found'; false; fi",
            label = "daemon start",
        )
        if (!result.isSuccess) throw AbdException.CommandFailed(result.output)
        if (awaitRunning(abd)) return@changing true
        if (disabledOrRemoved()) return@changing false
        throw AbdException.TimedOut()
    }

    override suspend fun turnOnAgain(): Boolean {
        val result = rootShell.run("rm -f $MODULE_DIR/disable", label = "module enable")
        if (!result.isSuccess) throw AbdException.CommandFailed(result.error.ifBlank { result.output })
        return startDaemon()
    }

    override suspend fun restartDevice() {
        // The shell may not live to say how it went.
        rootShell.run("svc power reboot || reboot", label = "reboot", timeoutSeconds = REBOOT_TIMEOUT_SECONDS)
    }

    private suspend fun disabledOrRemoved(): Boolean = rootShell.run("[ -e $MODULE_DIR/disable ] || [ -e $MODULE_DIR/remove ]").isSuccess

    private fun bundledVersionCode(): Long = context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode

    /** A development build can carry different daemon bytes under the same version. */
    private suspend fun verifyDaemon(): ModuleState {
        val bundled = bundledHash ?: withContext(ioDispatcher) {
            bundledDaemonHash(context.applicationInfo.sourceDir, Build.SUPPORTED_ABIS)
        }?.also { bundledHash = it } ?: return ModuleState.Ready
        val result = rootShell.run("sha256sum $ABD")
        val installed = result.output.substringBefore(' ').trim()
        if (!result.isSuccess || !SHA256.matches(installed)) return ModuleState.Unavailable
        return if (installed.equals(bundled, ignoreCase = true)) ModuleState.Ready else ModuleState.Mismatched
    }

    private companion object {
        val SHA256 = Regex("[0-9a-fA-F]{64}")

        /** Flashing unpacks and checks the whole module. */
        const val INSTALL_TIMEOUT_SECONDS = 300
        const val REBOOT_TIMEOUT_SECONDS = 30
    }
}

/** The installer looks for bin/<arch>/abd, using these architecture names. */
private val archByAbi = mapOf("arm64-v8a" to "arm64", "x86_64" to "x64")

internal fun bundledDaemonHash(apkPath: String, abis: Array<String>): String? = ZipFile(apkPath).use { zip ->
    // APKs built without a flashable module still compare by version code.
    if (zip.entries().asSequence().none { it.name.startsWith("bin/") && it.name.endsWith("/abd") }) return null
    val arch = abis.firstNotNullOfOrNull { archByAbi[it] } ?: throw IOException("Unsupported daemon architecture")
    val entry = zip.getEntry("bin/$arch/abd") ?: throw IOException("Bundled daemon missing for this architecture")
    zip.getInputStream(entry).use(::sha256)
}

private fun sha256(input: InputStream): String {
    val hash = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(64 * 1024)
    while (true) {
        val count = input.read(buffer)
        if (count < 0) break
        hash.update(buffer, 0, count)
    }
    return hash.digest().joinToString("") { "%02x".format(it) }
}

/** The supervisor starts the daemon after the service script returns. */
internal suspend fun awaitRunning(abd: AbdClient): Boolean = withTimeoutOrNull(5_000L) {
    while (!abd.status().running) delay(500L)
    true
} ?: false

/**
 * A shell command that flashes the module in [apkPath] with the installer of
 * whichever root manager is present.
 *
 * Magisk is tested first because `magisk -V` asks the running daemon, so it
 * only succeeds while Magisk is the active root. The daemons of KernelSU and
 * APatch are found by their files, which can outlive a switch to another
 * manager; KernelSU wins if both exist.
 */
internal fun installCommand(apkPath: String): String {
    val apk = apkPath.shellQuoted()
    return """
        if magisk -V >/dev/null 2>&1; then
            magisk --install-module $apk 2>&1
        elif [ -x /data/adb/ksud ]; then
            /data/adb/ksud module install $apk 2>&1
        elif [ -x /data/adb/apd ]; then
            /data/adb/apd module install $apk 2>&1
        else
            echo "No module installer found (KernelSU, Magisk or APatch)."
            exit 1
        fi
    """.trimIndent()
}
