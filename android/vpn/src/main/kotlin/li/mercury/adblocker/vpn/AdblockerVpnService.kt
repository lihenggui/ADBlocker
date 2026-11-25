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

import android.app.Notification
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import li.mercury.adblocker.core.common.di.ApplicationScope
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.UserDataRepository
import li.mercury.adblocker.core.data.repository.VpnAppRepository
import li.mercury.adblocker.core.engine.exclusiveAppUid
import li.mercury.adblocker.core.engine.personalAppIdentities
import li.mercury.adblocker.core.model.APP_LIST_PERMISSION
import li.mercury.adblocker.core.model.VpnState
import timber.log.Timber
import java.io.IOException
import java.net.Inet6Address
import java.net.InetAddress
import javax.inject.Inject

/**
 * Routes the chosen apps into the engine (docs/engine-jni.md). Started by
 * the app, or by the system for Always-on VPN; what it does is
 * [VpnSession]'s, and this is what the system gives it.
 */
@AndroidEntryPoint
class AdblockerVpnService : VpnService() {
    @Inject internal lateinit var running: RunningVpn

    @Inject internal lateinit var recovery: VpnBatteryRecovery

    @Inject internal lateinit var engine: VpnEngine

    @Inject internal lateinit var notifications: VpnNotifications

    @Inject lateinit var vpnApps: VpnAppRepository

    @Inject lateinit var https: HttpsRepository

    @Inject lateinit var userData: UserDataRepository

    @Inject lateinit var dataChanges: DataChanges

    @Inject
    @ApplicationScope
    lateinit var scope: CoroutineScope

    private lateinit var session: VpnSession

    private val host = object : VpnHost {
        override fun uidOf(packageName: String): Int? = packageManager.exclusiveAppUid(packageName)

        override val ipv6: Boolean get() = underlay.ipv6
        override val networkHandle: Long? get() = underlay.network?.networkHandle
        override val dnsServers: List<String> get() = underlay.dnsServers

        override fun personalAppIdentities() = packageManager.personalAppIdentities(applicationInfo.uid)

        override fun appListAccess() = ContextCompat.checkSelfPermission(this@AdblockerVpnService, APP_LIST_PERMISSION) == PackageManager.PERMISSION_GRANTED

        override fun establish(apps: Set<String>, ipv6: Boolean, virtualDns: Boolean): Int? {
            val builder = Builder()
                .setSession(applicationInfo.loadLabel(packageManager).toString())
                .setMtu(MTU)
                .addAddress(IPV4_ADDRESS, if (virtualDns) 30 else 32)
                .addRoute("0.0.0.0", 0)
                .setMetered(false)
            // Without IPv6 in the VPN, the system answers the chosen apps'
            // IPv6 connections as unreachable at once, and they use IPv4.
            if (ipv6) builder.addAddress(IPV6_ADDRESS, if (virtualDns) 126 else 128).addRoute("::", 0)
            if (virtualDns) {
                builder.addDnsServer(IPV4_DNS)
                if (ipv6) builder.addDnsServer(IPV6_DNS)
            }
            notifications.openApp()?.let(builder::setConfigureIntent)
            var allowed = 0
            for (app in apps) {
                try {
                    builder.addAllowedApplication(app)
                    allowed++
                } catch (_: PackageManager.NameNotFoundException) {
                    // Removed since it was looked up: the broadcast of its removal follows.
                }
            }
            // A VPN that allows no app would take the traffic of every app.
            check(allowed > 0) { "none of the apps is installed" }
            return builder.establish()?.detachFd()
        }

        override fun close(fd: Int) {
            try {
                ParcelFileDescriptor.adoptFd(fd).close()
            } catch (e: IOException) {
                Timber.w("vpn: close: %s", e.javaClass.simpleName)
            }
        }

        // The VPN follows the system default network. Android routes protected sockets outside the tunnel.
        override fun protect(fd: Int): Boolean = this@AdblockerVpnService.protect(fd)

        override fun prepared(): Boolean = prepare(this@AdblockerVpnService) == null

        override val alwaysOn: Boolean
            get() = asked("always on") { isAlwaysOn }

        override val lockdown: Boolean
            get() = asked("lockdown") { isLockdownEnabled }

        override fun show(shown: Shown?) {
            if (shown == null) notifications.cancelRevoked()
            enterForeground(notifications.running(shown))
        }

        override fun filteringEstablished() = recovery.established()

        override fun filteringEnded(interrupted: Boolean) {
            if (interrupted) recovery.interrupted() else recovery.clear()
        }

        override fun noticeRevoked() = notifications.postRevoked()

        override fun finish(startId: Int) {
            ServiceCompat.stopForeground(this@AdblockerVpnService, ServiceCompat.STOP_FOREGROUND_REMOVE)
            stopSelfResult(startId)
        }
    }

    private data class Underlay(val network: Network?, val ipv6: Boolean, val dnsServers: List<String> = emptyList())

    private fun underlay(network: Network?, properties: LinkProperties?): Underlay = Underlay(
        network,
        properties?.reachesIpv6() == true,
        properties?.dnsServers.orEmpty().mapNotNull { address ->
            if (address.isAnyLocalAddress || address.isMulticastAddress) return@mapNotNull null
            val host = address.hostAddress?.substringBefore('%') ?: return@mapNotNull null
            if (address is Inet6Address) {
                val scope = address.scopeId.takeIf { it > 0 }?.let { "%$it" }.orEmpty()
                "[$host$scope]:53"
            } else {
                "$host:53"
            }
        }.distinct(),
    )

    @Volatile
    private var underlay = Underlay(null, false)

    private val network = object : ConnectivityManager.NetworkCallback() {
        private var physicalDefault: Network? = null

        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            // Android also reports the VPN we own. Its virtual DNS addresses
            // are not upstream resolvers.
            if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) {
                physicalDefault = network
            }
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
            if (network != physicalDefault) return
            val current = underlay(network, properties)
            if (current != underlay) {
                underlay = current
                session.networkChanged()
            }
        }

        override fun onLost(network: Network) {
            if (physicalDefault == network) physicalDefault = null
            if (underlay.network == network) {
                underlay = Underlay(null, false)
                session.networkChanged()
            }
        }
    }

    private val packages = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            // An app updated keeps its uid: nothing for the VPN or the engine.
            if (intent.getBooleanExtra(Intent.EXTRA_REPLACING, false)) return
            session.packagesChanged()
        }
    }

    private val clock = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            session.clockChanged()
        }
    }

    override fun onCreate() {
        super.onCreate()
        notifications.createChannels()
        session = VpnSession(
            host = host,
            engine = engine,
            vpnApps = vpnApps,
            reload = { https.reload() },
            method = userData.userData.map { it.method }.distinctUntilChanged(),
            changes = dataChanges.changes,
            running = running,
            ownPackage = packageName,
            scope = scope,
        )
        running.session = session
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REMOVED)
            addDataScheme("package")
        }
        // The system's broadcasts reach a receiver that is not exported.
        ContextCompat.registerReceiver(this, packages, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        val clockEvents = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_TIME_CHANGED)
        }
        ContextCompat.registerReceiver(this, clock, clockEvents, ContextCompat.RECEIVER_NOT_EXPORTED)
        val connectivity = getSystemService(ConnectivityManager::class.java)
        val active = connectivity.activeNetwork?.takeIf {
            connectivity.getNetworkCapabilities(it)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN) == true
        }
        underlay = underlay(active, active?.let(connectivity::getLinkProperties))
        connectivity.registerDefaultNetworkCallback(network)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            // SERVICE_INTERFACE: the system starts the VPN for Always-on VPN.
            ACTION_START, SERVICE_INTERFACE -> if (enterForeground(notifications.running(null))) {
                // Undispatched: the starts reach the session in the order they came.
                scope.launch(start = CoroutineStart.UNDISPATCHED) { session.start(startId) }
            } else {
                recovery.clear()
                running.status.update { it.copy(state = VpnState.Failed) }
                stopSelfResult(startId)
            }

            else -> scope.launch(start = CoroutineStart.UNDISPATCHED) {
                session.unrecognizedStart(startId) { stopSelfResult(startId) }
            }
        }
        return START_NOT_STICKY
    }

    /** Another VPN app was given the VPN, or the user ended it in the system's settings. */
    override fun onRevoke() {
        // Not super's, which stops the service before the engine has let go.
        scope.launch(start = CoroutineStart.UNDISPATCHED) { session.revoked() }
    }

    override fun onDestroy() {
        unregisterReceiver(packages)
        unregisterReceiver(clock)
        getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(network)
        scope.launch(start = CoroutineStart.UNDISPATCHED) { session.destroyed() }
        if (running.session === session) running.session = null
        super.onDestroy()
    }

    private fun enterForeground(notification: Notification): Boolean = try {
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED else 0
        ServiceCompat.startForeground(this, VpnNotifications.RUNNING_ID, notification, type)
        true
    } catch (e: Exception) {
        // Refused, as when the user's consent was taken back: the VPN is not started.
        Timber.w("vpn: not in the foreground: %s", e.javaClass.simpleName)
        false
    }

    private inline fun asked(what: String, ask: () -> Boolean): Boolean = try {
        ask()
    } catch (e: RuntimeException) {
        Timber.w("vpn: %s: %s", what, e.javaClass.simpleName)
        false
    }

    internal companion object {
        private const val ACTION_START = "li.mercury.adblocker.vpn.START"
        const val MTU = 9000

        // Private addresses for the interface: the engine answers in place
        // of every server, whatever these are.
        const val IPV4_ADDRESS = "10.120.38.1"
        const val IPV6_ADDRESS = "fd8b:5f3a:9c21::1"
        const val IPV4_DNS = "10.120.38.2"
        const val IPV6_DNS = "fd8b:5f3a:9c21::2"

        fun startIntent(context: Context): Intent = Intent(context, AdblockerVpnService::class.java).setAction(ACTION_START)
    }
}

/**
 * Whether these are of a network that reaches the internet over IPv6: a
 * global address and a default route. Without it the VPN carries no IPv6,
 * and the system refuses the chosen apps' IPv6 connections at once, rather
 * than the engine after trying each onward.
 */
internal fun LinkProperties.reachesIpv6(): Boolean = linkAddresses.any { it.address.isGlobalIpv6() } &&
    routes.any { it.isDefaultRoute && it.destination.address is Inet6Address }

private fun InetAddress.isGlobalIpv6(): Boolean = this is Inet6Address &&
    !isLinkLocalAddress &&
    !isSiteLocalAddress &&
    !isLoopbackAddress &&
    // Unique local addresses, fc00::/7, reach no further than their site.
    (address[0].toInt() and 0xfe) != 0xfc
