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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.root.bounded
import li.mercury.adblocker.core.data.root.fakeBin
import li.mercury.adblocker.core.data.root.script
import li.mercury.adblocker.core.data.root.sh
import li.mercury.adblocker.core.data.root.shellQuoted
import li.mercury.adblocker.core.model.DeviceCheck
import li.mercury.adblocker.core.model.DeviceCheckKind
import li.mercury.adblocker.core.testing.unexpected
import java.util.zip.GZIPOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class RootDeviceRepositoryTest {
    private val release = "6.6.50-android15"
    private val config = """
        CONFIG_IP_MULTIPLE_TABLES=y
        CONFIG_NETFILTER_XT_MARK=y
        CONFIG_NETFILTER_XT_TARGET_TPROXY=y
        CONFIG_NETFILTER_XT_MATCH_MULTIPORT=y
        CONFIG_NETFILTER_XT_MATCH_OWNER=y
        CONFIG_NETFILTER_XT_MATCH_QUOTA2=y
        CONFIG_IP_NF_MANGLE=y
    """.trimIndent()

    private suspend fun check(config: String?, missingTool: String? = null, corrupt: Boolean = false): DeviceCheck {
        val bin = fakeBin()
        bin.script("uname", "echo $release")
        for (tool in listOf("timeout", "ip", "iptables", "iptables-restore", "ip6tables", "ip6tables-restore")) {
            if (tool != missingTool) bin.script(tool, "exit 0")
        }
        val gzip = bin.resolve("config.gz").apply { deleteOnExit() }
        if (config != null) GZIPOutputStream(gzip.outputStream()).bufferedWriter().use { it.appendLine(config) }
        if (corrupt) {
            val contents = gzip.readBytes()
            // A CRC error occurs after zcat has had a chance to print the options.
            val crc = contents.size - 8
            contents[crc] = (contents[crc].toInt() xor 1).toByte()
            gzip.writeBytes(contents)
        }
        val repository = RootDeviceRepository(
            context = ContextWrapper(null),
            rootShell = object : RootShell(Dispatchers.Unconfined) {
                override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
                    assertEquals("kernel check", label)
                    val fixtureCommand = command
                        .replace("\"/system/bin/\$tool\"", "${bin.absolutePath.shellQuoted()}/\"\$tool\"")
                        .replace("/proc/config.gz", gzip.absolutePath.shellQuoted())
                    // Android's printf is an external command and cannot accept a very large argument.
                    val shell = """
                        export PATH=${bin.absolutePath.shellQuoted()}:"${'$'}PATH"
                        printf() { command env printf "${'$'}@"; }
                        $fixtureCommand
                    """.trimIndent()
                    val ran = sh(bounded(shell, timeoutSeconds))
                    return CommandResult(ran.code == 0, ran.code, ran.output, "")
                }
            },
            module = unexpected(),
        )
        return repository.check(DeviceCheckKind.Kernel)
    }

    @Test
    fun givenTcpCaptureFeaturesWithoutUdpReject_whenChecked_thenRootModeIsAvailable() = runTest {
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, true, release), check(config))
    }

    @Test
    fun givenLargeKernelConfig_whenCheckedThroughTheShell_thenRequiredOptionsAreFound() = runTest {
        val large = "# CONFIG_OTHER_KERNEL_OPTION is not set\n".repeat(5_000) + config
        assertTrue(large.length > 128 * 1024)
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, true, release), check(large))
    }

    @Test
    fun givenOwnerMissing_whenChecked_thenRootModeIsUnavailable() = runTest {
        val noOwner = config.replace("CONFIG_NETFILTER_XT_MATCH_OWNER=y", "CONFIG_UNRELATED_OWNER=y")
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, false, release), check(noOwner))
    }

    @Test
    fun givenQuota2Missing_whenChecked_thenRootModeIsUnavailable() = runTest {
        val noQuota2 = config.replace("CONFIG_NETFILTER_XT_MATCH_QUOTA2=y", "# CONFIG_NETFILTER_XT_MATCH_QUOTA2 is not set")
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, false, release), check(noQuota2))
    }

    @Test
    fun givenOwnerBuiltAsModule_whenChecked_thenAvailabilityIsDeferred() = runTest {
        val modular = config.replace("CONFIG_NETFILTER_XT_MATCH_OWNER=y", "CONFIG_NETFILTER_XT_MATCH_OWNER=m")
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, true, null), check(modular))
    }

    @Test
    fun givenConfigUnavailable_whenToolsArePresent_thenAvailabilityIsDeferred() = runTest {
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, true, null), check(config = null))
    }

    @Test
    fun givenPartiallyReadConfig_whenZcatFails_thenAvailabilityIsDeferred() = runTest {
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, true, null), check(config, corrupt = true))
    }

    @Test
    fun givenNoRequiredOptions_whenConfigIsReadCompletely_thenRootModeIsUnavailable() = runTest {
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, false, release), check("# No required options are enabled"))
    }

    @Test
    fun givenRequiredToolMissing_whenChecked_thenRootModeIsUnavailable() = runTest {
        assertEquals(DeviceCheck(DeviceCheckKind.Kernel, false, release), check(config, missingTool = "timeout"))
    }
}
