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
import androidx.core.app.LocaleManagerCompat
import dagger.hilt.android.qualifiers.ApplicationContext
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.model.DeviceCheck
import li.mercury.adblocker.core.model.DeviceCheckKind
import javax.inject.Inject

/**
 * Whether this device can run the module: checked before it is installed,
 * from the app, with what module/customize.sh checks again when flashing.
 */
interface DeviceRepository {
    suspend fun check(kind: DeviceCheckKind): DeviceCheck

    fun regionCode(): String?
}

internal class RootDeviceRepository @Inject constructor(
    @ApplicationContext private val context: Context,
    private val rootShell: RootShell,
    private val module: ModuleRepository,
) : DeviceRepository {
    override fun regionCode(): String? = LocaleManagerCompat.getSystemLocales(context)[0]?.country?.takeIf { it.isNotEmpty() }

    override suspend fun check(kind: DeviceCheckKind): DeviceCheck = when (kind) {
        DeviceCheckKind.Root -> try {
            rootShell.run("true", label = "root check")
            DeviceCheck(kind, passed = true, detail = module.rootManager())
        } catch (_: AbdException.NoRoot) {
            DeviceCheck(kind, passed = false, detail = null)
        }

        // The root module supports Android 12 and later; the VPN runs from Android 10.
        DeviceCheckKind.AndroidVersion -> DeviceCheck(kind, passed = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S, detail = "Android ${Build.VERSION.RELEASE}")

        DeviceCheckKind.Architecture -> {
            val abi = Build.SUPPORTED_ABIS.firstOrNull { it == "arm64-v8a" || it == "x86_64" }
            DeviceCheck(kind, passed = abi != null, detail = abi ?: Build.SUPPORTED_ABIS.firstOrNull())
        }

        DeviceCheckKind.Kernel -> kernel()
    }

    /**
     * Check the kernel options used by the packet filter without changing the
     * device's network rules. If the configuration cannot be read, or a feature
     * is modular, its availability is checked when the daemon installs rules.
     */
    private suspend fun kernel(): DeviceCheck {
        val result = try {
            rootShell.run(KERNEL_CHECK, label = "kernel check")
        } catch (_: AbdException.NoRoot) {
            return DeviceCheck(DeviceCheckKind.Kernel, passed = false, detail = null)
        }
        val lines = result.output.trim().lines()
        val release = lines.first().takeIf(String::isNotEmpty)
        if (!result.isSuccess || release == null) return DeviceCheck(DeviceCheckKind.Kernel, passed = false, detail = release)

        val options = lines.drop(1)
        // A passing result without a version still needs verification at startup.
        if (options.lastOrNull() != CONFIG_COMPLETE) return DeviceCheck(DeviceCheckKind.Kernel, passed = true, detail = null)
        val enabled = options.dropLast(1).toSet()
        val builtIn = OPTIONS.all { "CONFIG_$it=y" in enabled }
        val available = OPTIONS.all { "CONFIG_$it=y" in enabled || "CONFIG_$it=m" in enabled }
        return DeviceCheck(DeviceCheckKind.Kernel, passed = available, detail = release.takeIf { builtIn || !available })
    }

    private companion object {
        const val CONFIG_COMPLETE = "config-complete"

        // Keep these IPv4 prerequisites in sync with module/customize.sh.
        val OPTIONS = listOf(
            "NETFILTER_XT_MATCH_OWNER",
            "NETFILTER_XT_TARGET_TPROXY",
            "NETFILTER_XT_MARK",
            "NETFILTER_XT_MATCH_MULTIPORT",
            "NETFILTER_XT_MATCH_QUOTA2",
            "IP_MULTIPLE_TABLES",
            "IP_NF_MANGLE",
        )

        // Filtering in the pipe keeps the decompressed config out of argv and
        // only trusts the option lines after zcat has finished successfully.
        val KERNEL_CHECK = """
            uname -r || exit 1
            for tool in timeout ip iptables iptables-restore ip6tables ip6tables-restore; do
                [ -x "/system/bin/${'$'}tool" ] || exit 1
            done
            { zcat /proc/config.gz 2>/dev/null && echo && echo $CONFIG_COMPLETE; } |
                grep -E '^(CONFIG_(${OPTIONS.joinToString("|")})=[ym]|$CONFIG_COMPLETE)${'$'}'
            exit 0
        """.trimIndent()
    }
}
