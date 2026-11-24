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

package li.mercury.adblocker.core.engine

import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.os.Process
import android.os.UserHandle
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.PERSONAL_APP_UIDS
import li.mercury.adblocker.core.common.PER_USER_RANGE
import li.mercury.adblocker.core.common.dispatchers.ADBlockerDispatchers.IO
import li.mercury.adblocker.core.common.dispatchers.Dispatcher
import timber.log.Timber
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import javax.inject.Inject
import javax.inject.Singleton

data class PersonalAppIdentity(val key: String, val uid: Int, val members: Set<String>) {
    fun isExcluded(exclusions: Set<String>): Boolean = isPersonalAppExcluded(key, members, exclusions)
}

fun isPersonalAppExcluded(key: String, members: Iterable<String>, exclusions: Set<String>): Boolean = key in exclusions || members.any { it in exclusions }

/** One stable identity per personal app UID that Android can attribute. */
fun PackageManager.personalAppIdentities(ownUid: Int): List<PersonalAppIdentity> {
    if (Process.myUserHandle() != UserHandle.getUserHandleForUid(0)) return emptyList()
    return getInstalledPackages(0)
        .mapNotNull { info -> info.applicationInfo?.uid?.takeIf { it in PERSONAL_APP_UIDS && it != ownUid }?.let { it to info } }
        .groupBy({ it.first }, { it.second })
        .mapNotNull { (uid, infos) ->
            val names = infos.mapTo(mutableSetOf()) { it.packageName }
            if (getPackagesForUid(uid)?.toSet() != names) return@mapNotNull null
            val shared = infos.map { it.sharedUserId }.distinct().singleOrNull()
            val key = shared?.let { "@$it" } ?: names.singleOrNull() ?: return@mapNotNull null
            PersonalAppIdentity(key, uid, names)
        }
}

/** A VPN can select only whole UIDs. Keep an unselected sibling outside the tunnel. */
fun PackageManager.exclusiveAppUid(packageName: String): Int? = try {
    getPackageUid(packageName, 0).takeIf { uid -> uid % PER_USER_RANGE in PERSONAL_APP_UIDS && getPackagesForUid(uid)?.singleOrNull() == packageName }
} catch (_: PackageManager.NameNotFoundException) {
    null
}

/** The VPN service, while it runs: what the engine needs of it. */
interface VpnBinding {
    fun protect(fd: Int): Boolean

    /** The stack ended by itself: the service stops, and the apps fall back to the network. */
    fun stopped(code: Int)
}

/** A command or stream of the engine ended with [exit], as `abd` would have. */
class EngineExit(val exit: Int, val stderr: String) : Exception("exit $exit")

/**
 * The engine of this process, opened on first use and kept for the life of
 * the process (docs/engine-jni.md). Commands work whether or not the VPN
 * runs; the VPN service starts and stops the stack.
 */
@Singleton
class EngineHost @Inject internal constructor(
    @ApplicationContext private val context: Context,
    private val api: EngineApi,
    private val userCa: UserCa,
    @Dispatcher(IO) private val ioDispatcher: CoroutineDispatcher,
) {
    private val opening = Mutex()

    // Written under [opening]; stop() reads it without, on another thread.
    @Volatile
    private var handle = 0L

    /** Set by the VPN service while it runs. */
    @Volatile
    var vpn: VpnBinding? = null

    /**
     * Whether this build carries the engine: without it, there is no VPN
     * mode. The first call loads the library, which is large: off the main
     * thread.
     */
    suspend fun available(): Boolean = withContext(ioDispatcher) { api.available }

    @Volatile
    private var personalAppSnapshot: List<PersonalAppIdentity>? = null

    /** Share the package inventory with native reloads during a serialized VPN refresh. */
    suspend fun <T> withPersonalAppSnapshot(apps: List<PersonalAppIdentity>, refresh: suspend () -> T): T {
        personalAppSnapshot = apps
        return try {
            refresh()
        } finally {
            personalAppSnapshot = null
        }
    }

    private val callbacks = object : Callbacks {
        override fun protect(fd: Int): Boolean = vpn?.protect(fd) ?: false

        override fun ownerUid(protocol: Int, local: ByteArray, localPort: Int, remote: ByteArray, remotePort: Int): Int = try {
            val connectivity = context.getSystemService(ConnectivityManager::class.java)
            connectivity.getConnectionOwnerUid(
                protocol,
                InetSocketAddress(InetAddress.getByAddress(local), localPort),
                InetSocketAddress(InetAddress.getByAddress(remote), remotePort),
            )
        } catch (e: Exception) {
            // Not known: the connection is handed on unread.
            Timber.w("owner uid: %s", e.javaClass.simpleName)
            -1
        }

        override fun uidOf(packageName: String): Int = context.packageManager.exclusiveAppUid(packageName) ?: -1

        override fun personalAppIdentities(exclusions: String): String {
            val excluded = exclusions.lineSequence().toSet()
            val apps = personalAppSnapshot ?: context.packageManager.personalAppIdentities(context.applicationInfo.uid)
            return apps.filterNot { it.isExcluded(excluded) }
                .joinToString("\n") { "${it.uid} ${it.key}" }
        }

        override fun userCaInstalled(): Boolean = userCa.installed()

        override fun stopped(code: Int) {
            Timber.w("engine stack stopped: %d", code)
            vpn?.stopped(code)
        }
    }

    private suspend fun handle(): Long = opening.withLock {
        if (handle == 0L) {
            check(api.available) { "no engine in this build" }
            val dir = File(context.filesDir, "engine").apply { mkdirs() }
            handle = api.open(dir.absolutePath, userCa.systemRoots(), systemLanguageTags(context).toTypedArray(), callbacks)
            if (handle == 0L) throw EngineExit(1, "the engine could not open")
        }
        handle
    }

    /** One command, as `abd <args>` with [stdin]; throws [EngineExit] unless it exits with 0. */
    suspend fun command(args: List<String>, stdin: ByteArray? = null): String = withContext(ioDispatcher) {
        val result = api.command(handle(), args.toTypedArray(), stdin)
        if (result.exit != 0) throw EngineExit(result.exit, result.stderr.decodeToString().take(MAX_STDERR))
        result.stdout.decodeToString()
    }

    /** A streamed command, a line at a time; stopping the collection cancels it. */
    fun stream(args: List<String>): Flow<String> = callbackFlow {
        val h = handle()
        val writeTimeout = WATCH_WRITE_TIMEOUT_MILLIS.takeIf { args.firstOrNull() == "watch" }
        val id = api.stream(h, args.toTypedArray(), EngineLineSink(channel, writeTimeout))
        awaitClose { api.cancel(h, id) }
    }.flowOn(ioDispatcher)

    /** Starts the stack on [tunFd], which the engine now owns; 0 when it started. */
    suspend fun start(tunFd: Int, ipv6: Boolean, dnsServers: List<String>): Int = withContext(ioDispatcher) { api.start(handle(), tunFd, ipv6, dnsServers.toTypedArray()) }

    suspend fun stop() = withContext(ioDispatcher) {
        val open = handle
        if (open != 0L) api.stop(open)
    }

    private companion object {
        const val MAX_STDERR = 16 * 1024
        const val WATCH_WRITE_TIMEOUT_MILLIS = 30_000L
    }
}

/** The library's functions, behind an interface so that tests can stand in for it. */
interface EngineApi {
    val available: Boolean

    fun open(dataDir: String, roots: Array<ByteArray>, languages: Array<String>, callbacks: Callbacks): Long

    fun command(handle: Long, args: Array<String>, stdin: ByteArray?): Result

    fun stream(handle: Long, args: Array<String>, sink: LineSink): Long

    fun cancel(handle: Long, id: Long)

    fun start(handle: Long, tunFd: Int, ipv6: Boolean, dnsServers: Array<String>): Int

    fun stop(handle: Long)
}

internal class NativeEngineApi @Inject constructor() : EngineApi {
    override val available: Boolean by lazy {
        try {
            System.loadLibrary("abd_vpn")
            true
        } catch (_: UnsatisfiedLinkError) {
            false
        }
    }

    override fun open(dataDir: String, roots: Array<ByteArray>, languages: Array<String>, callbacks: Callbacks) = Engine.open(dataDir, roots, languages, callbacks)

    override fun command(handle: Long, args: Array<String>, stdin: ByteArray?) = Engine.command(handle, args, stdin)

    override fun stream(handle: Long, args: Array<String>, sink: LineSink) = Engine.stream(handle, args, sink)

    override fun cancel(handle: Long, id: Long) = Engine.cancel(handle, id)

    override fun start(handle: Long, tunFd: Int, ipv6: Boolean, dnsServers: Array<String>) = Engine.start(handle, tunFd, ipv6, dnsServers)

    override fun stop(handle: Long) = Engine.stop(handle)
}
