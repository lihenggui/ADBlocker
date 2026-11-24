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

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import li.mercury.adblocker.core.model.ListCategory

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

    /**
     * Initial suggestions only. The confirmed choices are stored by the
     * engine. Enabled are the engine's defaults for [languageTags]: the
     * general lists, and those for a primary language subtag among them.
     */
    fun setupLists(languageTags: List<String>): Map<String, Boolean> {
        val languages = languageTags.mapTo(mutableSetOf()) { it.trim().split('-', '_').first().lowercase() }
        return entries.mapNotNull { entry ->
            val enabled = entry.defaultEnabled || entry.defaultLanguages.any { it in languages }
            if (enabled || entry.offerInSetup) entry.url to enabled else null
        }.toMap()
    }

    fun byUrl(url: String): CatalogEntry? = entries.firstOrNull { it.url == url }
}
