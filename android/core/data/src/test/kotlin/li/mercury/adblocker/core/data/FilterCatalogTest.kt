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

package li.mercury.adblocker.core.data

import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class FilterCatalogTest {
    @Test
    fun givenSharedZoneCases_whenResolvingRegions_thenTheResultsMatchTheEngine() {
        val cases = File(checkNotNull(System.getProperty("timeZoneCases")))
        cases.forEachLine { line ->
            val (id, region) = line.split('\t')
            assertEquals(region.takeUnless { it == "-" }, FilterCatalog.timeZoneRegion(id), id)
        }
    }

    @Test
    fun givenALanguageRegionOrTimeZone_whenSuggestingLists_thenEachCanEnableTheSameDefaults() {
        val general = FilterCatalog.setupLists(listOf("en-US"), "Etc/UTC").filterValues { it }.keys
        val chinese = setOf("easylist_china", "adguard_chinese", "awavenue", "anti_ad")
        val expected = general + FilterCatalog.entries.filter { it.id in chinese }.map { it.url }
        for ((locales, zone) in listOf(
            listOf("zh") to "Etc/UTC",
            listOf("en-US", "en-Hans-CN") to "Etc/UTC",
            listOf("en-HK") to "Etc/UTC",
            listOf("en-MO") to "Etc/UTC",
            listOf("en-TW") to "Etc/UTC",
            listOf("en-US") to "Asia/Shanghai",
            listOf("en-US") to "Asia/Hong_Kong",
            listOf("en-US") to "Asia/Macau",
            listOf("en-US") to "Asia/Taipei",
        )) {
            assertEquals(expected, FilterCatalog.setupLists(locales, zone).filterValues { it }.keys, "$locales $zone")
        }
        assertEquals(general, FilterCatalog.setupLists(listOf("en-x-CN"), "Europe/Brussels").filterValues { it }.keys)
    }
}
