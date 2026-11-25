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

import android.icu.util.TimeZone
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import li.mercury.adblocker.core.model.ListCategory
import java.util.Locale

/** A list offered by the bundled catalog. */
@Serializable
data class CatalogEntry(
    val id: String,
    val url: String,
    val title: String,
    val category: ListCategory,
    val cosmetic: Boolean = false,
    /** Primary language subtags, lowercase, of the devices the list is a default for. */
    val defaultLanguages: Set<String> = emptySet(),
    val defaultRegions: Set<String> = emptySet(),
    val defaultEnabled: Boolean = false,
    val offerInSetup: Boolean = false,
    val descriptions: Map<String, String> = emptyMap(),
)

@Serializable
private data class CatalogDocument(val lists: List<CatalogEntry>)

/** The same bundled data supplies the engine's standalone defaults. */
object FilterCatalog {
    private val json = Json { ignoreUnknownKeys = true }

    val entries: List<CatalogEntry> by lazy {
        val resource = checkNotNull(javaClass.getResourceAsStream("/filter-catalog.json"))
        resource.bufferedReader().use { json.decodeFromString<CatalogDocument>(it.readText()).lists }
    }

    private val timeZoneRegions: Map<String, String> by lazy {
        val resource = checkNotNull(javaClass.getResourceAsStream("/regional-time-zones.tsv"))
        resource.bufferedReader().useLines { lines ->
            lines.associate { it.substringBefore('\t') to it.substringAfter('\t') }
        }
    }

    internal fun timeZoneRegion(timeZoneId: String): String? {
        // ICU assigns countries to some shared zones, and its data version
        // can differ from the engine's. Use the engine's single-country
        // table to check ICU's answer and cover any version differences.
        val expected = timeZoneRegions[timeZoneId] ?: return null
        val platform = try {
            TimeZone.getRegion(timeZoneId)
        } catch (_: IllegalArgumentException) {
            return expected
        }
        return platform.takeIf { it == expected } ?: expected
    }

    /**
     * Initial suggestions only. The confirmed choices are stored by the
     * engine. The general lists and the lists for the device's languages
     * or regions are enabled.
     */
    fun setupLists(localeTags: List<String>, timeZoneId: String): Map<String, Boolean> {
        val tags = localeTags.map { it.trim().replace('_', '-') }
        val languages = tags.map { it.substringBefore('-').lowercase() }.toSet()
        val regions = tags.mapTo(mutableSetOf()) { Locale.forLanguageTag(it).country }
        timeZoneRegion(timeZoneId)?.let(regions::add)
        return entries.mapNotNull { entry ->
            val enabled = entry.defaultEnabled ||
                entry.defaultLanguages.any { it in languages } ||
                entry.defaultRegions.any { it in regions }
            if (enabled || entry.offerInSetup) entry.url to enabled else null
        }.toMap()
    }

    fun byUrl(url: String): CatalogEntry? = entries.firstOrNull { it.url == url }

    /** The root module can be older than the app and use an earlier default address. */
    internal fun canBeDefault(url: String): Boolean = when (url) {
        "https://filters.adtidy.org/extension/ublock/filters/11.txt",
        "https://filters.adtidy.org/extension/ublock/filters/224.txt",
        -> true

        else -> byUrl(url)?.let { it.defaultEnabled || it.defaultLanguages.isNotEmpty() } == true
    }
}
