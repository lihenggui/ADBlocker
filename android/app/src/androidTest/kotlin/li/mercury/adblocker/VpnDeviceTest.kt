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

package li.mercury.adblocker

import android.app.UiAutomation
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.InetAddresses
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.os.Process
import androidx.test.platform.app.InstrumentationRegistry
import dagger.hilt.android.EntryPointAccessors
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import li.mercury.adblocker.core.engine.exclusiveAppUid
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.VpnState
import java.time.Duration
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.minutes

class VpnDeviceTest {
    @Test
    fun vpnAdmissionRequiresASinglePackageOwner() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val pm = context.packageManager
        assertEquals(context.applicationInfo.uid, pm.exclusiveAppUid(context.packageName))
        assertNull(pm.exclusiveAppUid("li.mercury.adblocker.missing.fixture"))
        val systemPackages = checkNotNull(pm.getPackagesForUid(Process.SYSTEM_UID))
        check(systemPackages.size > 1) { "The device check needs a shared system UID" }
        systemPackages.forEach { assertNull(pm.exclusiveAppUid(it)) }
    }

    @Test
    fun givenAllApps_whenVpnStartsAndAnAppIsExcluded_thenOnlyEligibleTrafficIsFiltered() = runDeviceTest {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val probe = instrumentation.context.packageName
        val services = EntryPointAccessors.fromApplication(context, VpnDeviceDependencies::class.java)
        val vpn = services.vpn()
        val abd = services.abd()
        assertNull(VpnService.prepare(context), "Grant VPN consent on the test emulator")
        services.userData().setFilterMethod(FilterMethod.Vpn)
        val initial = abd.appSelections()
        val rules = abd.userRules()
        check(initial.chosen.isEmpty() && initial.excluded.isEmpty() && rules.isEmpty()) { "The device check needs a fresh engine" }
        val dns = assertNotNull(fetch(context, probe, waitForVpn = false, dnsHost = "example.com"))
        val address = dns.lines().getOrNull(1)?.substringBefore(',')
        assertTrue(address != null && InetAddresses.isNumericAddress(address) && address != "0.0.0.0", dns)
        fun shell(command: String): String = ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(command))
            .bufferedReader().use { it.readText() }
        fun systemResponse(): String? {
            val (read, write) = instrumentation.uiAutomation.executeShellCommandRw("nc -w 8 -W 5 $address 80")
            return ParcelFileDescriptor.AutoCloseOutputStream(write).use { input ->
                ParcelFileDescriptor.AutoCloseInputStream(read).bufferedReader().use { output ->
                    input.write("GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n".encodeToByteArray())
                    input.flush()
                    output.readLine()
                }
            }
        }
        assertTrue(shell("id -u").trim().toInt() < 10_000)
        val baseline = systemResponse()
        assertTrue(baseline?.startsWith("HTTP/") == true, "System HTTP baseline: $baseline")
        suspend fun https(routed: Boolean) = fetch(context, probe, waitForVpn = routed, httpsAddress = address)

        try {
            assertEquals("false 200", https(false))
            abd.setUserRules("||example.com^\n")
            abd.setFilteringScope(FilteringScope.All)
            vpn.start()
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }
            assertEquals(FilteringScope.All, abd.status().filteringScope)
            assertTrue(probe !in abd.appSelections().chosen)
            assertEquals("true failed", https(true))
            assertEquals(baseline, systemResponse(), "System UID must keep direct HTTP")
            val blockedDns = assertNotNull(fetch(context, probe, waitForVpn = true, dnsHost = "example.com"))
            assertEquals("0.0.0.0", blockedDns.lines()[1], blockedDns)

            services.apps().setFiltered(probe, filtered = false)
            assertEquals("false 200", https(false), "An excluded app must regain its unfiltered network")
            assertTrue(probe in abd.appSelections().excluded)
            assertEquals(baseline, systemResponse(), "System UID stays outside the VPN after rebuild")

            services.apps().setFiltered(probe, filtered = true)
            assertEquals("true failed", https(true), "Re-inclusion must rebuild the VPN and restore filtering")
        } finally {
            vpn.stop()
            abd.configureExclusions(initial.excluded)
            abd.setFilteringScope(initial.scope)
            abd.setUserRules(rules)
        }
    }

    @Test
    fun givenSelectedApp_whenVpnStops_thenItsNetworkIsRestored() = runDeviceTest {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val probe = instrumentation.context.packageName
        val services = EntryPointAccessors.fromApplication(context, VpnDeviceDependencies::class.java)
        val vpn = services.vpn()
        val abd = services.abd()
        assertNull(VpnService.prepare(context), "Grant VPN consent on the test emulator")
        services.userData().setFilterMethod(FilterMethod.Vpn)
        check(abd.apps().isEmpty() && abd.userRules().isEmpty()) { "The device check needs a fresh installation" }
        val dns = assertNotNull(fetch(context, probe, waitForVpn = false, dnsHost = "example.com"))
        val address = dns.lines().getOrNull(1)?.substringBefore(',')
        assertTrue(address != null && InetAddresses.isNumericAddress(address) && address != "0.0.0.0", dns)
        suspend fun https(waitForVpn: Boolean) = fetch(context, probe, waitForVpn, httpsAddress = address)
        try {
            assertEquals("false 200", https(waitForVpn = false))
            abd.addApp(probe)
            abd.setUserRules("||example.com^\n")
            vpn.start()
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }
            val zone = ZoneId.systemDefault()
            val first = LocalDate.now(zone).minusDays(6)
            val starts = List(8) { first.plusDays(it.toLong()).atStartOfDay(zone).toEpochSecond() }
            val overview = abd.overview(starts)
            assertEquals(abd.status(), overview.status)
            assertEquals(abd.stats(starts), overview.stats)
            assertEquals(abd.lists(), overview.lists)
            assertEquals("true failed", https(waitForVpn = true))
            abd.pauseApp(probe, 1)
            val ends = checkNotNull(abd.apps().single { it.packageName == probe }.pausedUntil)
            assertEquals("true 200", https(waitForVpn = true))
            delay(Duration.between(Instant.now(), ends).toMillis().coerceAtLeast(0))
            // Only the app makes requests after expiry. A manual refresh would mask a stale timer.
            val afterExpiry = withTimeout(10_000) {
                var result = https(waitForVpn = true)
                while (result == "true 200") {
                    delay(200)
                    result = https(waitForVpn = true)
                }
                result
            }
            assertEquals("true failed", afterExpiry)
            abd.setUserRules("")
            assertEquals("true 200", https(waitForVpn = true))
            vpn.stop()
            assertEquals("false 200", https(waitForVpn = false))
        } finally {
            vpn.stop()
            abd.removeApp(probe)
            abd.setUserRules("")
        }
    }

    @Test
    fun givenRunningVpn_whenConsentIsRevoked_thenItStopsAndCanRestart() = runDeviceTest {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_USE_ACCESSIBILITY)
        } else {
            instrumentation.uiAutomation
        }
        val context = instrumentation.targetContext
        val probe = instrumentation.context.packageName
        val services = EntryPointAccessors.fromApplication(context, VpnDeviceDependencies::class.java)
        val vpn = services.vpn()
        val abd = services.abd()

        fun setConsent(mode: String) {
            val command = "cmd appops set ${context.packageName} ACTIVATE_VPN $mode"
            ParcelFileDescriptor.AutoCloseInputStream(automation.executeShellCommand(command))
                .bufferedReader().use { it.readText() }
        }

        assertNull(VpnService.prepare(context), "Grant VPN consent on the test emulator")
        services.userData().setFilterMethod(FilterMethod.Vpn)
        assertEquals(emptyList(), abd.apps())
        try {
            abd.addApp(probe)
            vpn.start()
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }

            setConsent("ignore")
            // prepare() asks Android to revalidate the running VPN's consent.
            assertNotNull(VpnService.prepare(context))
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.Revoked } }
            assertFailsWith<IllegalStateException> { vpn.start() }

            setConsent("allow")
            assertNull(VpnService.prepare(context))
            vpn.start()
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }
        } finally {
            vpn.stop()
            abd.removeApp(probe)
            setConsent("allow")
        }
    }
}

internal fun runDeviceTest(test: suspend () -> Unit) = runTest(timeout = 2.minutes) {
    // Android callbacks and pause expiry use real time.
    withContext(Dispatchers.IO) { test() }
}

internal suspend fun fetch(
    context: Context,
    probe: String,
    waitForVpn: Boolean,
    dnsHost: String? = null,
    httpsAddress: String? = null,
): String? = withTimeout(15_000) {
    val reply = CompletableDeferred<String?>()
    val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            reply.complete(resultData)
        }
    }
    val intent = Intent().setComponent(ComponentName(probe, "li.mercury.adblocker.test.VpnProbe"))
    intent.putExtra("waitForVpn", waitForVpn)
    dnsHost?.let { intent.putExtra("dnsHost", it) }
    httpsAddress?.let { intent.putExtra("httpsAddress", it) }
    context.sendOrderedBroadcast(intent, null, receiver, null, 0, null, null)
    reply.await()
}
