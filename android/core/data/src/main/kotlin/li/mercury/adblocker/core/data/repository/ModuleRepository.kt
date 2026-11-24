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
import li.mercury.adblocker.core.data.root.ModulePaths.DAEMON
import li.mercury.adblocker.core.data.root.ModulePaths.DIRECT_DIR
import li.mercury.adblocker.core.data.root.ModulePaths.LOCATE
import li.mercury.adblocker.core.data.root.ModulePaths.MODULE_DIR
import li.mercury.adblocker.core.data.root.ModulePaths.MODULE_ID
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

/**
 * The root module that does the filtering: flashed in a root manager
 * (KernelSU, Magisk or APatch), or installed by this app with root alone.
 */
interface ModuleRepository {
    /**
     * Where the app stands with root and the module in use. A module a root
     * manager has yet to apply needs a restart only while none that matches
     * this app is in use.
     */
    suspend fun state(): ModuleState

    /**
     * Whether the module in use is to be replaced with this app's without
     * the user asking: it is older than this app's, or the same version
     * built differently, and it is on. Installing turns a module on, so one
     * that is turned off or marked for removal is left as it is. So is one
     * newer than this app's, whose data this app's daemon may not read, and
     * one a root manager replaces at the next restart.
     *
     * @throws AbdException if root is refused.
     */
    suspend fun needsUpdate(): Boolean

    /** The installed module's version name, or null if it is not installed or root is missing. */
    suspend fun installedVersion(): String?

    /** Which root manager is installed: "KernelSU", "Magisk", "APatch", or null. */
    suspend fun rootManager(): String?

    /**
     * Installs the module this app's own APK carries, with root alone, and
     * starts it: no restart, and no root manager's installer. A module in a
     * root manager is brought up to date where it is and stays the manager's;
     * without one the module goes into a directory of this app's, and no
     * manager lists it. Returns whether it is active at once, which it is
     * whenever this returns.
     *
     * @throws AbdException if root is missing or the installer fails: a
     * [AbdException.CommandFailed] then carries what the installer said was
     * wrong, in its own words, for the screens to show.
     */
    suspend fun installBundled(): Boolean

    /**
     * Whether this app installed the module in use, and so is what removes
     * it. One flashed in a root manager is removed there. False without root.
     */
    suspend fun isInstalledByApp(): Boolean

    /**
     * Removes a module this app installed: stops the daemon, has the apps
     * stop trusting the authority, destroys its key and deletes the module
     * with everything it kept.
     *
     * @throws AbdException.CommandFailed if the module is not this app's, or
     * could not be removed; it is then still installed.
     */
    suspend fun remove()

    /**
     * Starts the daemon through the module's own service script, as at boot,
     * and waits a few seconds for it to run. A module this app installed
     * that turned itself off is turned on again, as no root manager can.
     * Returns false if a module in a root manager is disabled or marked for
     * removal there, and throws [AbdException.TimedOut] if the service did
     * not become ready.
     */
    suspend fun startDaemon(): Boolean

    /**
     * Starts the daemon of a module this app installed without the user
     * having asked, where nothing started it when the device did: no root
     * manager ran the module's launcher. Unlike [startDaemon] it turns
     * nothing on. Returns false, having started nothing, for a module in a
     * root manager, one that turned itself off, and a daemon that runs.
     *
     * @throws AbdException if root is refused or the daemon does not start.
     */
    suspend fun startUnasked(): Boolean

    /**
     * Turns the module on again after its boot guard turned it off, as the
     * root manager's switch would, and starts the daemon. Returns false when
     * the module is marked for removal in a root manager.
     */
    suspend fun turnOnAgain(): Boolean
}

// A singleton, so that the bundled daemon is hashed once, and the installed one
// again only when its file changes.
@Singleton
internal class RootModuleRepository(
    private val context: Context,
    private val rootShell: RootShell,
    private val abd: AbdClient,
    private val changes: DataChanges,
    private val userData: UserDataRepository,
    private val ioDispatcher: CoroutineDispatcher,
) : ModuleRepository {
    @Inject
    constructor(
        @ApplicationContext context: Context,
        rootShell: RootShell,
        root: RootTransport,
        changes: DataChanges,
        userData: UserDataRepository,
        @Dispatcher(IO) ioDispatcher: CoroutineDispatcher,
    ) : this(
        context,
        rootShell,
        root.fixedAbdClient(),
        changes,
        userData,
        ioDispatcher,
    )

    @Volatile private var bundledHash: String? = null
    private val installedDaemon = InstalledDaemonHash(rootShell)

    /** Which root manager answers does not change while the app runs; a missing one may be granted later, so it is asked again. */
    @Volatile private var rootManagerFound: String? = null

    override suspend fun state(): ModuleState = suspendRunCatching {
        // Whose a module that is ready is, is remembered for the next boot.
        probe().state.also { if (it == ModuleState.Ready) ownedByApp() }
    }.getOrElse {
        if (it is AbdException.NoRoot) ModuleState.NoRoot else ModuleState.Unavailable
    }

    override suspend fun needsUpdate(): Boolean {
        val found = probe()
        // Mismatched says, too, that no module waits in a root manager for a restart.
        return found.state == ModuleState.Mismatched && found.versionCode != null && found.versionCode <= bundledVersionCode() && !disabledOrRemoved()
    }

    /** The state of the module in use, and its version where it has one that can be read. */
    private class Found(val state: ModuleState, val versionCode: Long? = null)

    private suspend fun probe(): Found {
        val result = rootShell.run(
            """
            $LOCATE
            if [ -d $PENDING_UPDATE_DIR ]; then
                echo $PENDING
            elif [ -e $PENDING_UPDATE_DIR ] || [ -L $PENDING_UPDATE_DIR ]; then
                exit 1
            fi
            if [ ! -e $MODULE_DIR ] && [ ! -L $MODULE_DIR ]; then
                echo $ABSENT
            else
                [ -x $DAEMON ] && sed -n 's/^versionCode=/version /p' $MODULE_PROP && { stat -c "$STAT_PREFIX$STAT_FORMAT" $DAEMON 2>/dev/null || true; }
            fi
            """.trimIndent(),
            label = "module state",
        )
        if (!result.isSuccess) return Found(ModuleState.Unavailable)
        val probe = result.output.trim().lines().map(String::trim)
        val pending = probe.first() == PENDING
        val inUse = if (pending) probe.drop(1) else probe

        // A module waiting in a root manager replaces the one in use at the
        // next boot. Where none in use is this app's, that restart comes
        // before anything else is installed.
        fun orRestart(state: ModuleState) = if (pending) ModuleState.NeedsRestart else state
        val code = when (val output = inUse.firstOrNull()) {
            null -> return Found(ModuleState.Unavailable)
            ABSENT -> return Found(orRestart(ModuleState.NotInstalled))
            else -> output.removePrefix("version ").toLongOrNull() ?: return Found(ModuleState.Unavailable)
        }
        if (code != bundledVersionCode()) return Found(orRestart(ModuleState.Mismatched), code)
        return when (val verified = verifyDaemon(daemonFileKey(inUse))) {
            ModuleState.Mismatched -> Found(orRestart(verified), code)
            else -> Found(verified, code)
        }
    }

    override suspend fun installedVersion(): String? = try {
        rootShell.run("$LOCATE; sed -n 's/^version=//p' $MODULE_PROP", label = "module version")
            .output.lineSequence().firstOrNull()?.trim()?.ifEmpty { null }
    } catch (_: AbdException.NoRoot) {
        null
    }

    override suspend fun rootManager(): String? = rootManagerFound ?: try {
        rootShell.run(
            "if magisk -V >/dev/null 2>&1; then echo Magisk; elif [ -x /data/adb/ksud ]; then echo KernelSU; " +
                "elif [ -x /data/adb/apd ]; then echo APatch; fi",
            label = "root manager",
        ).output.trim().ifEmpty { null }?.also { rootManagerFound = it }
    } catch (_: AbdException.NoRoot) {
        null
    }

    override suspend fun installBundled(): Boolean {
        val result = rootShell.run(installCommand(context.applicationInfo.sourceDir), label = "module install", timeoutSeconds = INSTALL_TIMEOUT_SECONDS)
        if (!result.isSuccess) throw AbdException.CommandFailed(installerMessage(result.output))
        // The installer leaves the module in use: one that is not this
        // app's now was not installed.
        if (state() != ModuleState.Ready) throw AbdException.CommandFailed("Could not verify the installed module")
        return true
    }

    override suspend fun isInstalledByApp(): Boolean = try {
        ownedByApp()
    } catch (_: AbdException.NoRoot) {
        false
    }

    /** Asks root whose the module in use is, and remembers the answer for where root is not to be asked. */
    private suspend fun ownedByApp(): Boolean {
        val owned = rootShell.run("$LOCATE; [ $MODULE_DIR = $DIRECT_DIR ] && [ -f $DIRECT_DIR/direct.sh ]", label = "module owner").isSuccess
        userData.setModuleInstalledByApp(owned)
        return owned
    }

    override suspend fun remove() = changes.changing {
        // The script of the module itself: it removes what that version kept.
        val result = rootShell.run(
            "$LOCATE; [ $MODULE_DIR = $DIRECT_DIR ] && sh $DIRECT_DIR/direct.sh remove </dev/null 2>&1",
            label = "module remove",
            timeoutSeconds = REMOVE_TIMEOUT_SECONDS,
        )
        if (!result.isSuccess) throw AbdException.CommandFailed(result.output.ifBlank { "The module was not removed" })
        userData.setModuleInstalledByApp(false)
    }

    override suspend fun startDaemon(): Boolean = changes.changing {
        // Only its own boot guard turns off a module this app installed, and
        // no root manager has a switch for it. A switch that could not be
        // cleared is found by the check that follows.
        rootShell.run("$LOCATE; [ $MODULE_DIR != $DIRECT_DIR ] || rm -f $DIRECT_DIR/disable", label = "module enable")
        start()
    }

    override suspend fun startUnasked(): Boolean {
        // A module in a root manager is the manager's to start, and a daemon
        // that runs was started by the launcher a manager ran.
        if (!ownedByApp() || abd.status().running) return false
        // Not turned on again here: its boot guard turned it off for the user to see.
        return changes.changing { start() }
    }

    /** Runs the service script of a module that is on, and waits for its daemon. */
    private suspend fun start(): Boolean {
        if (disabledOrRemoved()) return false
        // The script leaves the supervisor running in the background; with
        // its output detached, the shell returns once it has been started.
        val script = "$MODULE_DIR/service.sh"
        val result = rootShell.run(
            "$LOCATE; if [ -f $script ]; then sh $script </dev/null >/dev/null 2>&1; else echo 'service.sh not found'; false; fi",
            label = "daemon start",
        )
        if (!result.isSuccess) throw AbdException.CommandFailed(result.output)
        if (awaitRunning(abd)) return true
        if (disabledOrRemoved()) return false
        throw AbdException.TimedOut()
    }

    override suspend fun turnOnAgain(): Boolean {
        val result = rootShell.run("$LOCATE; rm -f $MODULE_DIR/disable", label = "module enable")
        if (!result.isSuccess) throw AbdException.CommandFailed(result.error.ifBlank { result.output })
        return startDaemon()
    }

    private suspend fun disabledOrRemoved(): Boolean = rootShell.run("$LOCATE; [ -e $MODULE_DIR/disable ] || [ -e $MODULE_DIR/remove ]", label = "module switch").isSuccess

    private fun bundledVersionCode(): Long = context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode

    /** A development build can carry different daemon bytes under the same version. */
    private suspend fun verifyDaemon(fileKey: String?): ModuleState {
        val bundled = bundledHash ?: withContext(ioDispatcher) {
            bundledDaemonHash(context.applicationInfo.sourceDir, Build.SUPPORTED_ABIS)
        }?.also { bundledHash = it } ?: return ModuleState.Ready
        val installed = installedDaemon.hash(fileKey) ?: return ModuleState.Unavailable
        return if (installed.equals(bundled, ignoreCase = true)) ModuleState.Ready else ModuleState.Mismatched
    }

    private companion object {
        /** Installing unpacks and checks the whole module. */
        const val INSTALL_TIMEOUT_SECONDS = 300

        /** Removing waits for the daemon to stop, and for the packet filter's locks. */
        const val REMOVE_TIMEOUT_SECONDS = 120

        /** What the probe of the module prints first for a module waiting for a restart, and for none in use. */
        const val PENDING = "pending"
        const val ABSENT = "absent"
    }
}

/**
 * What `stat` prints, in the same shell call as the module's version, to tell
 * whether the installed daemon is the file that was hashed: its inode, size
 * and times change when a module update replaces it.
 */
private const val STAT_PREFIX = "daemon-file "
private const val STAT_FORMAT = "%i %s %y %z"

/** What `stat` said of the installed daemon after the version line of the module probe, if it said. */
internal fun daemonFileKey(probe: List<String>): String? = probe.drop(1).firstOrNull { it.startsWith(STAT_PREFIX) }?.removePrefix(STAT_PREFIX)

/** The SHA-256 of the installed daemon, read again only when its file's [key] is not the one hashed. */
internal class InstalledDaemonHash(private val rootShell: RootShell) {
    @Volatile private var known: Pair<String, String>? = null

    /** Null if it cannot be read. Without a [key] nothing is kept: the file is hashed every time. */
    suspend fun hash(key: String?): String? {
        known?.takeIf { key != null && it.first == key }?.let { return it.second }
        val result = rootShell.run("$LOCATE; sha256sum $DAEMON", label = "module daemon")
        val hash = result.output.substringBefore(' ').trim()
        if (!result.isSuccess || !SHA256.matches(hash)) return null
        known = key?.let { it to hash }
        return hash
    }

    private companion object {
        val SHA256 = Regex("[0-9a-fA-F]{64}")
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
 * What the installer said went wrong: the lines it marks with "!"
 * (module/customize.sh, module/direct.sh). The rest of what it printed is
 * what it did and what the tools it ran listed, with the paths of the
 * package and of its staging directory, and is not passed on.
 */
internal fun installerMessage(output: String): String = output.lineSequence()
    .map(String::trim)
    .filter { it.startsWith("!") }
    .joinToString("\n") { it.removePrefix("!").trim() }
    .ifEmpty { "The installer failed" }

/**
 * A shell command that installs the module in [apkPath] with the module's
 * own installer, module/direct.sh. The script and what it installs both come
 * out of the installed APK, which only the package manager replaces; the
 * script is run from a copy in a directory of root's.
 */
internal fun installCommand(apkPath: String): String {
    val apk = apkPath.shellQuoted()
    return "(umask 077; d=\$(mktemp -d /data/adb/.$MODULE_ID-install.XXXXXX) || exit 1; trap 'rm -rf \"\$d\"' EXIT; " +
        "unzip -p $apk direct.sh >\"\$d/direct.sh\" && sh \"\$d/direct.sh\" install $apk) 2>&1"
}
