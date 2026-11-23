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

package li.mercury.adblocker.hook

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ModuleMountsTest {
    private val apex = "/apex/com.android.conscrypt/cacerts"
    private val user = "/data/misc/user/0/cacerts-added"

    // Lines of /proc/<pid>/mountinfo of Chrome, filtered, and of Settings,
    // not filtered, on a Pixel emulator with Android 15.
    private val data =
        "2946 2625 254:46 / /data rw,nosuid,nodev,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel"
    private val conscrypt =
        "2915 2625 254:24 / /apex/com.android.conscrypt ro,nodev,noatime - ext4 /dev/block/dm-24 ro,dirsync,seclabel"
    private val systemCopy = "3472 2915 254:46 /adb/adblocker/store $apex " +
        "ro,nosuid,nodev,noexec,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel"
    private val userCopy = "3617 2946 254:46 /adb/adblocker/user-store $user " +
        "ro,nosuid,nodev,noexec,noatime master:34 - ext4 /dev/block/dm-46 rw,seclabel"
    private val filtered = listOf(data, conscrypt, systemCopy, userCopy).joinToString("\n")
    private val unfiltered = listOf(data, conscrypt).joinToString("\n")

    private fun isSystemCopy(table: String, point: String = apex) = ModuleMounts.isModuleCopy(table, point, ModuleMounts.SYSTEM_COPY)

    @Test
    fun givenAFilteredAppsTable_whenAsked_thenBothStoresAreTheModulesCopies() {
        assertTrue(isSystemCopy(filtered))
        assertTrue(ModuleMounts.isModuleCopy(filtered, user, ModuleMounts.USER_COPY))
        assertFalse(ModuleMounts.isModuleCopy(filtered, user, ModuleMounts.SYSTEM_COPY))
    }

    @Test
    fun givenAnUnfilteredAppsTable_whenAsked_thenNoStoreIsTheModulesCopy() {
        assertFalse(isSystemCopy(unfiltered))
        assertFalse(ModuleMounts.isModuleCopy(unfiltered, user, ModuleMounts.USER_COPY))
    }

    @Test
    fun givenAnotherModulesCopyOfTheStore_whenAsked_thenItIsNotTheModulesCopy() {
        val bound = "3500 2915 254:46 /adb/modules/certificates/cacerts $apex ro - ext4 /dev/block/dm-46 rw"
        val tmpfs = "3500 2915 0:93 / $apex rw - tmpfs tmpfs rw,seclabel"

        assertFalse(isSystemCopy(listOf(data, conscrypt, bound).joinToString("\n")))
        assertFalse(isSystemCopy(listOf(data, conscrypt, tmpfs).joinToString("\n")))
    }

    @Test
    fun givenAMountOverTheModulesCopyOrADirectoryAboveIt_whenAsked_thenItIsNotTheModulesCopy() {
        val over = "3700 2915 0:93 / $apex rw - tmpfs tmpfs rw,seclabel"
        val above = "3700 2625 0:93 / /apex/com.android.conscrypt rw - tmpfs tmpfs rw,seclabel"
        val elsewhere = "3700 2946 254:46 /data /data/data rw - ext4 /dev/block/dm-46 rw"

        assertFalse(isSystemCopy("$filtered\n$over"))
        assertFalse(isSystemCopy("$filtered\n$above"))
        assertTrue(isSystemCopy("$filtered\n$elsewhere"))
    }

    @Test
    fun givenTheSameDirectoryOfAnotherFileSystem_whenAsked_thenItIsNotTheModulesCopy() {
        val other = "3472 2915 7:1 /adb/adblocker/store $apex ro - ext4 /dev/block/loop1 rw"

        assertFalse(isSystemCopy(listOf(data, conscrypt, other).joinToString("\n")))
        assertFalse(isSystemCopy(listOf(conscrypt, systemCopy).joinToString("\n")), "no /data to compare with")
    }

    @Test
    fun givenTheStoreOfAndroidBefore14_whenMounted_thenItIsTheModulesCopy() {
        val legacy = "/system/etc/security/cacerts"
        val copy = "3472 2625 254:46 /adb/adblocker/store $legacy ro,nosuid,nodev,noexec - ext4 /dev/block/dm-46 rw"

        assertTrue(isSystemCopy(listOf(data, copy).joinToString("\n"), legacy))
        assertFalse(isSystemCopy(listOf(data, copy).joinToString("\n")))
    }

    @Test
    fun givenAPathWithAnEscapedSpace_whenAsked_thenItIsReadAsWritten() {
        val point = "/tmp/a store"
        val copy = "3472 2625 254:46 /adb/adblocker/store /tmp/a\\040store ro - ext4 /dev/block/dm-46 rw"

        assertTrue(isSystemCopy(listOf(data, copy).joinToString("\n"), point))
    }
}
