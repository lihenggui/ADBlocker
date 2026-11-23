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

package li.mercury.adblocker.vpn

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import li.mercury.adblocker.core.common.result.suspendRunCatching
import li.mercury.adblocker.core.data.readSection
import li.mercury.adblocker.core.data.repository.VpnAppRepository
import li.mercury.adblocker.core.engine.PersonalAppIdentity
import li.mercury.adblocker.core.engine.VpnBinding
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import timber.log.Timber
import java.time.Duration
import java.time.Instant
import java.util.concurrent.atomic.AtomicBoolean

/** What a session needs of the VPN service, to which the system gives the VPN. */
internal interface VpnHost {
    /** The uid of [packageName]; null if it is not installed. */
    fun uidOf(packageName: String): Int?

    /**
     * Whether the network beneath reaches the internet over IPv6. Without
     * it the VPN carries no IPv6 (docs/engine-jni.md).
     */
    val ipv6: Boolean

    val networkHandle: Long?

    val dnsServers: List<String> get() = emptyList()

    /**
     * A VPN for [apps] and nothing else, built as docs/engine-jni.md says,
     * with IPv6 if [ipv6]: its descriptor, detached. Null when the system
     * gives none, as when another VPN app was given the VPN since.
     */
    fun establish(apps: Set<String>, ipv6: Boolean, virtualDns: Boolean): Int?

    /** All attributable personal app identities except ADBlocker. */
    fun personalAppIdentities(): List<PersonalAppIdentity>

    /** Closes a descriptor from [establish] that the engine never took. */
    fun close(fd: Int)

    fun protect(fd: Int): Boolean

    /** Whether the user's consent holds: false once another VPN app was given the VPN. */
    fun prepared(): Boolean

    val alwaysOn: Boolean

    val lockdown: Boolean

    /** Shows the ongoing notification, in the foreground; [shown] is null while starting. */
    fun show(shown: Shown?)

    fun filteringEstablished() = Unit

    fun filteringEnded(interrupted: Boolean) = Unit

    /** Tells the user, once, that another VPN app took the VPN. */
    fun noticeRevoked()

    /** Leaves the foreground and stops the service, unless it was started again after [startId]. */
    fun finish(startId: Int)
}

/** What the ongoing notification says. */
internal data class Shown(val apps: Int, val status: ReadState<DaemonStatus>)

/**
 * The work of one VPN service: builds the VPN for the chosen apps, hands
 * it to the engine, builds it again when the set of chosen apps changes,
 * and ends it. Whatever goes wrong ends it, and the VPN with it: the
 * chosen apps are then on the network, unfiltered.
 *
 * Every step that touches the engine runs under [RunningVpn.lock].
 */
internal class VpnSession(
    private val host: VpnHost,
    private val engine: VpnEngine,
    private val vpnApps: VpnAppRepository,
    /** Has the engine read the installed packages again. */
    private val reload: suspend () -> Unit,
    private val method: Flow<FilterMethod>,
    /** Told of every change the app makes to the engine's state. */
    private val changes: Flow<Unit>,
    private val running: RunningVpn,
    private val ownPackage: String,
    /** Outlives the service: an end that has begun is carried through. */
    private val scope: CoroutineScope,
    private val now: () -> Instant = Instant::now,
) {
    private var active = false
    private var startId = 0

    /** The apps, by package and uid, the VPN in place routes; empty while it has none to route. */
    private var routed: Map<String, Int>? = null
    private var appCount = 0

    /** Whether the VPN in place carries IPv6. */
    private var routedIpv6 = false
    private var routedNetwork: Long? = null
    private var routedDns: List<String> = emptyList()

    /**
     * Which stack the engine runs, counted up whenever one is stopped: a
     * stack that tells of its end after it was replaced or stopped is
     * no failure of the VPN.
     */
    @Volatile
    private var stack = 0
    private var watching: Job? = null
    private var pauseEnd: Job? = null

    // Conflated: however many changes come at once, one refresh follows.
    private val refreshes = Channel<Unit>(Channel.CONFLATED)
    private val reloadAsked = AtomicBoolean()

    private val binding = object : VpnBinding {
        override fun protect(fd: Int): Boolean = host.protect(fd)

        override fun stopped(code: Int) {
            // Called on an engine thread, which stopping the engine waits for.
            val which = stack
            scope.launch { stoppedByItself(code, which) }
        }
    }

    /** Starts the VPN, or checks the one running, for the service's start [startId]. */
    suspend fun start(startId: Int): Unit = running.lock.withLock {
        this.startId = startId
        if (active) {
            refreshLocked(starting = false)
            return@withLock
        }
        if (method.first() != FilterMethod.Vpn) {
            Timber.i("vpn: not started, the method in use is the root module")
            publish(VpnState.Off)
            host.finish(startId)
            return@withLock
        }
        active = true
        publish(VpnState.Starting)
        host.show(null)
        engine.binding = binding
        watch()
        if (refreshLocked(starting = true)) publish(VpnState.On)
    }

    suspend fun stop(): Unit = running.lock.withLock {
        if (active) {
            end(VpnState.Off)
        } else {
            host.filteringEnded(interrupted = false)
            publish(VpnState.Off)
        }
    }

    /** Builds the VPN again if the set of chosen apps changed. */
    suspend fun refresh() {
        running.lock.withLock { refreshLocked(starting = false) }
    }

    /** A package was installed or removed: the engine reads them again, and the VPN may change. */
    fun packagesChanged() {
        reloadAsked.set(true)
        refreshes.trySend(Unit)
    }

    /** The network beneath changed: the VPN may carry IPv6, or no longer. */
    fun networkChanged() {
        refreshes.trySend(Unit)
    }

    /** The system took the VPN away, and has deactivated it already. */
    suspend fun revoked(): Unit = running.lock.withLock { end(VpnState.Revoked) }

    /** The service ends without having been stopped by this session. */
    suspend fun destroyed(): Unit = running.lock.withLock {
        end(VpnState.Off, interrupted = !routed.isNullOrEmpty() && host.prepared())
    }

    private fun watch() {
        watching?.cancel()
        watching = scope.launch {
            launch { changes.collect { refreshes.trySend(Unit) } }
            launch {
                method.first { it != FilterMethod.Vpn }
                Timber.i("vpn: stopped, the method in use is now the root module")
                stop()
            }
            for (ignored in refreshes) refresh()
        }
    }

    /** Reads the chosen apps and builds the VPN again if their set changed; false once it ended. */
    private suspend fun refreshLocked(starting: Boolean): Boolean {
        if (!active) return false
        val selection = suspendRunCatching { vpnApps.appSelections() }
        val config = selection.getOrNull()
        if (config?.scope == FilteringScope.All) {
            val inventory = host.personalAppIdentities()
            val selected = inventory.filterNot { it.isExcluded(config.excluded) }
            val wanted = selected.flatMap { app -> app.members.map { it to app.uid } }.toMap()
            return engine.withPersonalAppSnapshot(inventory) {
                reloadIfAsked()
                refreshRoutes(wanted, selected.size)
            }
        }

        reloadIfAsked()
        val chosen = suspendRunCatching {
            selection.getOrThrow()
            vpnApps.apps()
        }.getOrElse {
            Timber.w("vpn: chosen apps: %s", it.javaClass.simpleName)
            // One that cannot start ends; one that runs goes on as it is.
            if (starting) {
                end(VpnState.Failed)
                return false
            }
            show()
            publish(running.status.value.state)
            return true
        }
        val wanted = allowedApps(chosen, ownPackage, host::uidOf)
        return refreshRoutes(wanted, wanted.size)
    }

    private suspend fun reloadIfAsked() {
        if (reloadAsked.getAndSet(false)) {
            suspendRunCatching { reload() }.onFailure { Timber.w("vpn: reload: %s", it.javaClass.simpleName) }
        }
    }

    private suspend fun refreshRoutes(wanted: Map<String, Int>, count: Int): Boolean {
        appCount = count
        val ipv6 = host.ipv6
        val network = host.networkHandle
        val dns = host.dnsServers
        val networkChanged = ipv6 != routedIpv6 || network != routedNetwork || dns != routedDns
        if (wanted != routed || (wanted.isNotEmpty() && networkChanged)) {
            if (!route(wanted, ipv6, dns)) return false
            routedNetwork = network
            routedDns = dns
        }
        show()
        publish(running.status.value.state)
        return true
    }

    /** Has the engine run a VPN for [wanted]; false if it could not, and the session ended. */
    private suspend fun route(wanted: Map<String, Int>, ipv6: Boolean, dns: List<String>): Boolean {
        if (wanted.isEmpty()) {
            // Never a VPN that allows no app: it would take the traffic of
            // every app. The service waits for an app to be chosen.
            stopStack()
            routed = wanted
            host.filteringEnded(interrupted = false)
            return true
        }
        val fd = try {
            host.establish(wanted.keys, ipv6, virtualDns = dns.isNotEmpty())
        } catch (e: Exception) {
            Timber.w("vpn: not built: %s", e.javaClass.simpleName)
            end(VpnState.Failed)
            return false
        }
        if (fd == null) {
            Timber.w("vpn: not built, the system gave no descriptor")
            end(if (host.prepared()) VpnState.Failed else VpnState.Revoked)
            return false
        }
        // The new VPN has taken the old one's place: the engine lets go of
        // the old descriptor before it is given the new one.
        stopStack()
        val code = withContext(NonCancellable) {
            try {
                engine.start(fd, ipv6, dns)
            } catch (e: Exception) {
                // Thrown before the engine had the descriptor, which takes it
                // once called and answers with a code: closed here, or the
                // VPN would stay with nobody reading it.
                Timber.w("vpn: engine: %s", e.javaClass.simpleName)
                host.close(fd)
                null
            }
        }
        if (code != 0) {
            if (code != null) Timber.w("vpn: the engine did not start: %d", code)
            end(VpnState.Failed)
            return false
        }
        routed = wanted
        routedIpv6 = ipv6
        host.filteringEstablished()
        Timber.i("vpn: built for %d apps, IPv6 %b", wanted.size, ipv6)
        return true
    }

    private suspend fun stopStack() {
        engine.stop()
        // After stop has returned, no end it could tell of is of the stack to come.
        stack++
    }

    private suspend fun show() {
        val status = readSection { vpnApps.status() }
        val daemon = status.valueOrNull
        val resumeAt = daemon?.takeIf { it.paused }?.resumeAt
        host.show(Shown(apps = appCount, status = status))
        // The engine ends a pause by itself and tells nobody: the
        // notification is read again then.
        pauseEnd?.cancel()
        if (resumeAt != null) {
            pauseEnd = scope.launch {
                delay(Duration.between(now(), resumeAt).toMillis().coerceAtLeast(0) + PAUSE_END_MARGIN_MS)
                refreshes.trySend(Unit)
            }
        }
    }

    private suspend fun stoppedByItself(code: Int, which: Int): Unit = running.lock.withLock {
        if (!active || which != stack) return@withLock
        Timber.w("vpn: the engine stopped by itself: %d", code)
        // Another VPN app taking the VPN ends the stack before onRevoke comes.
        end(if (host.prepared()) VpnState.Failed else VpnState.Revoked)
    }

    /** Ends the VPN: the engine lets go of it, and the service stops. Carried through once begun. */
    private suspend fun end(state: VpnState, interrupted: Boolean = false): Unit = withContext(NonCancellable) {
        if (!active) return@withContext
        active = false
        host.filteringEnded(interrupted)
        pauseEnd?.cancel()
        stopStack()
        routed = null
        if (engine.binding === binding) engine.binding = null
        publish(state)
        if (state == VpnState.Revoked) host.noticeRevoked()
        host.finish(startId)
        Timber.i("vpn: ended: %s", state)
        // Last: the end may have been reached from the watching itself.
        watching?.cancel()
    }

    private fun publish(state: VpnState) {
        val status = VpnStatus(state = state, alwaysOn = host.alwaysOn, lockdown = host.lockdown)
        running.status.update { status }
    }

    private companion object {
        /** After the end of a pause, so that the engine has ended it when it is read again. */
        const val PAUSE_END_MARGIN_MS = 1_000L
    }
}

/**
 * The apps the VPN routes: the chosen apps that are installed, by package
 * and uid, and never this app, whose own connections are the engine's.
 * A paused app is routed: the engine hands its connections on unread, so
 * pausing never builds the VPN again. A uid is part of what is routed: an
 * app installed again under a new one needs a new VPN.
 */
internal fun allowedApps(chosen: List<FilteredApp>, ownPackage: String, uidOf: (String) -> Int?): Map<String, Int> = chosen.asSequence()
    .filter { it.profileSerial == null && !it.packageName.startsWith('@') }
    .map { it.packageName }
    .filter { it != ownPackage }
    .distinct()
    .mapNotNull { packageName -> uidOf(packageName)?.let { packageName to it } }
    .toMap()
