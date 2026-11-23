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

import androidx.test.platform.app.InstrumentationRegistry
import dagger.hilt.android.EntryPointAccessors
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import li.mercury.adblocker.core.data.apps.InstalledAppsSource
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.FilterMethod
import org.junit.Assume.assumeTrue
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class WorkProfileDeviceTest {
    @Test
    fun givenManagedApp_whenReadThroughLauncherApps_thenOnlyInstalledInstancesAreShown(): Unit = runBlocking(Dispatchers.IO) {
        val fixture = fixture()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val source = InstalledAppsSource(context, Dispatchers.IO, this)
        val profile = WorkProfile(fixture.serial, setOf(fixture.packageName))

        val work = source.workApps(listOf(profile)).apps.single()
        assertEquals(fixture.serial, assertNotNull(work.work).serial)
        assertEquals(fixture.workKey, work.appKey)

        val personal = source.app(fixture.packageName)
        if (fixture.hasPersonal) {
            assertEquals(fixture.packageName, assertNotNull(personal).appKey)
            assertNull(personal.work)
        } else {
            assertNull(personal)
        }
    }

    @Test
    fun givenManagedApp_whenReadFromRootRepository_thenOnlyInstalledInstancesAreShown(): Unit = runBlocking(Dispatchers.IO) {
        val fixture = fixture()
        val args = InstrumentationRegistry.getArguments()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val services = EntryPointAccessors.fromApplication(context, VpnDeviceDependencies::class.java)
        assertEquals(FilterMethod.Root, services.userData().userData.first().method)

        val repository = services.apps()
        val matching = repository.apps().filter { entry -> entry.packageName == fixture.packageName }
        val expected = if (fixture.hasPersonal) listOf(fixture.packageName, fixture.workKey) else listOf(fixture.workKey)
        assertEquals(expected.sorted(), matching.map { it.appKey }.sorted())

        val work = assertNotNull(repository.app(fixture.workKey))
        assertEquals(fixture.serial, assertNotNull(work.app.work).serial)
        assertEquals(fixture.workKey, work.appKey)
        args.getString("workSelected")?.let { assertEquals(it.toBooleanStrict(), work.isFiltered) }

        val personal = repository.app(fixture.packageName)
        if (fixture.hasPersonal) {
            assertEquals(fixture.packageName, assertNotNull(personal).appKey)
            assertNull(personal.app.work)
            args.getString("personalSelected")?.let { assertEquals(it.toBooleanStrict(), personal.isFiltered) }
        } else {
            assertNull(personal)
        }
    }

    private fun fixture(): Fixture {
        val args = InstrumentationRegistry.getArguments()
        val packageName = args.getString("workPackage")
        val serial = args.getString("workSerial")?.toLongOrNull()
        assumeTrue("Specify the installed work test package and profile serial", packageName != null && serial != null)
        val hasPersonal = args.getString("workHasPersonal")?.toBooleanStrict() ?: false
        return Fixture(checkNotNull(packageName), checkNotNull(serial), hasPersonal)
    }

    private data class Fixture(val packageName: String, val serial: Long, val hasPersonal: Boolean) {
        val workKey: String get() = AppIdentity(packageName, serial).key
    }
}
