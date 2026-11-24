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

package li.mercury.adblocker.core.data.apps

import android.content.pm.PermissionInfo
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.model.APP_LIST_PERMISSION
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class AppPlatformInfoTest {
    private val application get() = RuntimeEnvironment.getApplication()
    private val platform get() = AndroidAppPlatformInfo(application, Dispatchers.Unconfined)

    private fun define(protection: Int) {
        shadowOf(application.packageManager).addPermissionInfo(
            PermissionInfo().apply {
                name = APP_LIST_PERMISSION
                packageName = "com.example.security"
                protectionLevel = protection
            },
        )
    }

    @Test
    fun givenASystemWithoutTheAppListPermission_whenChecked_thenTheListIsNotWithheld() = runTest {
        assertFalse(platform.appListWithheld())
    }

    @Test
    fun givenADangerousAppListPermissionNotGranted_whenChecked_thenTheListIsWithheld() = runTest {
        define(PermissionInfo.PROTECTION_DANGEROUS)

        assertTrue(platform.appListWithheld())
    }

    @Test
    fun givenTheAppListPermissionGranted_whenChecked_thenTheListIsNotWithheld() = runTest {
        define(PermissionInfo.PROTECTION_DANGEROUS)
        shadowOf(application).grantPermissions(APP_LIST_PERMISSION)

        assertFalse(platform.appListWithheld())
    }

    @Test
    fun givenAnAppListPermissionThatIsNotRuntime_whenChecked_thenTheListIsNotWithheld() = runTest {
        define(PermissionInfo.PROTECTION_NORMAL)

        assertFalse(platform.appListWithheld())
    }
}
