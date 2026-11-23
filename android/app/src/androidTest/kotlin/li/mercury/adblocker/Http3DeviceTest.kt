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

import android.os.Bundle
import android.os.ParcelFileDescriptor
import androidx.test.platform.app.InstrumentationRegistry
import dagger.hilt.android.EntryPointAccessors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.VpnState
import org.junit.Assume.assumeTrue
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class Http3DeviceTest {
    @Test
    fun givenASelectedApp_whenQuicIsRefused_thenOnlyTheAllowedRequestReachesTheTcpOrigin() = runTest {
        val argument = InstrumentationRegistry.getArguments().getString("ipv6")
        assumeTrue("Run scripts/check-http3-device.sh vpn to start the origin", argument != null)
        withContext(Dispatchers.IO) { checkRefusal(requireNotNull(argument).toBooleanStrict()) }
    }

    private suspend fun checkRefusal(ipv6: Boolean) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val services = EntryPointAccessors.fromApplication(instrumentation.targetContext, VpnDeviceDependencies::class.java)
        val packageName = "li.mercury.adblocker.check.http3"
        fun shell(command: String): String = ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(command))
            .bufferedReader().use { it.readText().trim() }

        suspend fun fetch(caller: String, probe: String): String {
            shell("run-as $caller rm -f files/result")
            shell("am start -S -W -n $caller/$packageName.Probe --ez ipv6 $ipv6 --es probe $probe")
            return withTimeout(12_000) {
                var result = shell("run-as $caller cat files/result 2>/dev/null")
                while (result.isEmpty() || result.contains("No such file")) {
                    delay(100)
                    result = shell("run-as $caller cat files/result 2>/dev/null")
                }
                result
            }
        }

        val vpn = services.vpn()
        val abd = services.abd()
        val userData = services.userData()
        val previousMethod = userData.userData.first().method
        userData.setFilterMethod(FilterMethod.Vpn)
        try {
            check(abd.apps().isEmpty() && abd.userRules().isEmpty()) { "The device check needs a fresh installation" }
            val previousUrls = abd.status().urlFiltering == true
            try {
                abd.setUrlFiltering(true)
                abd.addApp(packageName)
                abd.setUserRules("||fixture.example/ads/\n")
                vpn.start()
                withTimeout(15_000) { vpn.status.first { it.state == VpnState.On } }
                val control = fetch("$packageName.control", "fetch")
                assertEquals(setOf("h3 /ads/banner 200", "h3 /articles/today 200"), control.lines().toSet())
                if (ipv6 && abd.status().ipv6 == false) {
                    val status = Bundle().apply { putString("stream", "\nVPN has no IPv6 route\n") }
                    instrumentation.sendStatus(2, status)
                    return
                }
                val result = fetch(packageName, "dual-fetch")
                assertTrue(result.lines().any { it.matches(Regex("refused in [0-9]{1,3} ms")) }, result)
                assertTrue("tcp /ads/banner 403" in result && "tcp /articles/today 200" in result, result)
            } finally {
                vpn.stop()
                withTimeout(15_000) { vpn.status.first { it.state == VpnState.Off } }
                abd.setUserRules("")
                abd.removeApp(packageName)
                abd.setUrlFiltering(previousUrls)
            }
        } finally {
            userData.setFilterMethod(previousMethod)
        }
    }
}
