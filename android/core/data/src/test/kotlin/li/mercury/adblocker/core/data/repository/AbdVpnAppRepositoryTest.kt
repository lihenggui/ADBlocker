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

import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.fixedAbdClient
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.testing.unexpected
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class AbdVpnAppRepositoryTest {
    private val cases = File(checkNotNull(System.getProperty("protocolCases")))
    private val commands = mutableListOf<List<String>>()
    private var available = true
    private val transport = object : AbdTransport by unexpected() {
        override suspend fun run(args: List<String>, label: String, input: String?): String {
            commands += args
            if (!available) throw AbdException.EngineMissing()
            val fixture = when (args) {
                listOf("app", "selections", "--json") -> "app-selections.json"
                listOf("app", "list", "--json") -> "apps.json"
                listOf("status", "--json") -> "status-filtering-apps.json"
                else -> error("Unexpected command: $args")
            }
            return cases.resolve(fixture).readText()
        }
    }
    private val repository = AbdVpnAppRepository(transport.fixedAbdClient())

    @Test
    fun givenSavedChoicesAndEffectiveStateDiffer_whenRead_thenSelectionAppsAndStatusKeepTheirOwnValues() = runTest {
        val selections = repository.appSelections()
        assertEquals(FilteringScope.All, selections.scope)
        assertEquals(setOf("com.example.browser", "37:com.example.news"), selections.chosen)
        assertEquals(setOf("@com.example.suite", "com.example.shop"), selections.excluded)

        val apps = repository.apps().associateBy { it.packageName }
        assertEquals(setOf("com.example.browser", "com.example.news", "com.example.shop"), apps.keys)
        assertEquals(FilterLevel.Full, apps.getValue("com.example.browser").httpLevel)

        val status = repository.status()
        assertTrue(status.running)
        assertEquals(3, status.filteringApps)
        assertEquals(
            listOf(listOf("app", "selections", "--json"), listOf("app", "list", "--json"), listOf("status", "--json")),
            commands,
        )
    }

    @Test
    fun givenTheEngineIsUnavailable_whenSelectionIsRead_thenTheFailureIsPropagated() = runTest {
        available = false

        assertFailsWith<AbdException.EngineMissing> { repository.appSelections() }
        assertEquals(listOf(listOf("app", "selections", "--json")), commands)
    }
}
