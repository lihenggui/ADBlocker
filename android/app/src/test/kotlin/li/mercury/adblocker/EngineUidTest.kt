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

import android.app.Application
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import li.mercury.adblocker.core.engine.exclusiveAppUid
import li.mercury.adblocker.core.engine.personalAppIdentities
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowProcess
import kotlin.test.assertEquals
import kotlin.test.assertNull

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [37], application = Application::class)
class EngineUidTest {
    private val application = RuntimeEnvironment.getApplication()
    private val packages = application.packageManager

    @Test
    fun givenPersonalInventory_whenListingIdentities_thenOnlyAttributablePersonalAppUidsQualify() {
        val ownUid = 10_001
        ShadowProcess.setUid(ownUid)
        shadowOf(packages).removePackage(application.packageName)
        install("com.example.system", 9_999)
        install("com.example.first", 10_000)
        install("com.example.self", ownUid)
        install("com.example.last", 19_999)
        install("com.example.after", 20_000)
        install("com.example.work", 1_010_000)
        install("com.example.incomplete", 10_002)
        shadowOf(packages).setPackagesForUid(10_002, "com.example.incomplete", "com.example.hidden")

        assertEquals(
            mapOf("com.example.first" to 10_000, "com.example.last" to 19_999),
            packages.personalAppIdentities(ownUid).associate { it.key to it.uid },
        )
    }

    @Test
    fun givenProfileUids_whenLookingUpExclusiveApps_thenAppIdAndMembershipDetermineEligibility() {
        install("com.example.first", 1_010_000)
        install("com.example.last", 1_019_999)
        install("com.example.system", 1_009_999)
        install("com.example.after", 1_020_000)
        install("com.example.shared", 1_010_001)
        shadowOf(packages).setPackagesForUid(1_010_001, "com.example.shared", "com.example.sibling")

        assertEquals(1_010_000, packages.exclusiveAppUid("com.example.first"))
        assertEquals(1_019_999, packages.exclusiveAppUid("com.example.last"))
        assertNull(packages.exclusiveAppUid("com.example.system"))
        assertNull(packages.exclusiveAppUid("com.example.after"))
        assertNull(packages.exclusiveAppUid("com.example.shared"))
    }

    private fun install(name: String, uid: Int) {
        shadowOf(packages).installPackage(
            PackageInfo().apply {
                packageName = name
                applicationInfo = ApplicationInfo().apply {
                    packageName = name
                    this.uid = uid
                }
            },
        )
        shadowOf(packages).setPackagesForUid(uid, name)
    }
}
