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

import android.net.InetAddresses
import android.net.VpnService
import android.os.Bundle
import android.provider.Settings
import androidx.test.platform.app.InstrumentationRegistry
import dagger.hilt.android.EntryPointAccessors
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import li.mercury.adblocker.core.engine.exclusiveAppUid
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.VpnState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class VpnDnsDeviceTest {
    @Test
    fun givenSelectedClient_whenItResolvesHostNames_thenDnsFollowsThePrivateDnsMode() = runDeviceTest {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val probe = instrumentation.context.packageName
        val services = EntryPointAccessors.fromApplication(context, VpnDeviceDependencies::class.java)
        val vpn = services.vpn()
        val abd = services.abd()
        val strict = InstrumentationRegistry.getArguments().getString("strictPrivateDns") == "true"
        val mode = Settings.Global.getString(context.contentResolver, "private_dns_mode")
            ?: Settings.Global.getString(context.contentResolver, "private_dns_default_mode")
            ?: "opportunistic"
        assertEquals(if (strict) "hostname" else "opportunistic", mode)
        assertNull(VpnService.prepare(context), "Grant VPN consent on the test emulator")
        services.userData().setFilterMethod(FilterMethod.Vpn)
        check(abd.apps().isEmpty() && abd.userRules().isEmpty()) { "The device check needs a fresh installation" }

        val blocked = "example.org"
        val allowed = "example.com"
        val uid = context.packageManager.exclusiveAppUid(probe)
        suspend fun resolve(host: String, selected: Boolean): DnsAnswers {
            val result = assertNotNull(fetch(context, probe, waitForVpn = selected, dnsHost = host))
            val lines = result.lines()
            val answers = lines.joinToString(" / ")
            instrumentation.sendStatus(
                2,
                Bundle().apply {
                    putString("stream", "\nDNS mode=$mode uid=$uid selected=$selected host=$host answers=$answers\n")
                },
            )
            assertEquals(3, lines.size, result)
            val (usingVpn, ipv6) = lines[0].split(" ")
            assertEquals(selected, usingVpn.toBooleanStrict(), result)
            return DnsAnswers(lines[1].split(","), lines[2].split(",").takeIf { ipv6.toBooleanStrict() })
        }

        try {
            val unselected = resolve(blocked, selected = false)
            abd.addApp(probe)
            abd.setUserRules("||$blocked^\n")
            vpn.start()
            withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }

            val filtered = resolve(blocked, selected = true)
            val forwarded = resolve(allowed, selected = true)
            services.apps().configure(emptySet())
            val deselected = resolve(blocked, selected = false)
            unselected.assertRealAddresses()
            forwarded.assertRealAddresses()
            deselected.assertRealAddresses()
            if (strict) {
                filtered.assertRealAddresses()
            } else {
                assertEquals(listOf("0.0.0.0"), filtered.ipv4, "Selected app's A lookup")
                filtered.ipv6?.let { assertEquals(listOf("::"), it, "Selected app's AAAA lookup") }
            }
        } finally {
            vpn.stop()
            abd.removeApp(probe)
            abd.setUserRules("")
        }
    }

    private data class DnsAnswers(val ipv4: List<String>, val ipv6: List<String>?) {
        fun assertRealAddresses() {
            assertTrue(ipv4.all { InetAddresses.isNumericAddress(it) && it != "0.0.0.0" }, "A lookup: $ipv4")
            ipv6?.let { answers ->
                assertTrue(answers.all { InetAddresses.isNumericAddress(it) && it != "::" }, "AAAA lookup: $answers")
            }
        }
    }
}
