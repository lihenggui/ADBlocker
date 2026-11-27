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

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class CheckCommandTest {
    private val commands = mutableListOf<List<String>>()
    private val inputs = mutableListOf<String?>()
    private var legacyDaemon = false
    private val client = DefaultAbdClient(
        object : TransportSource {
            override suspend fun current() = object : AbdTransport {
                override suspend fun run(args: List<String>, label: String, input: String?): String {
                    commands += args
                    inputs += input
                    if (args.first() == "rules") {
                        if (legacyDaemon && args.last() == "--json") throw AbdException.ModuleOutdated()
                        return if (args.last() == "--json") """{"changed":true}""" else ""
                    }
                    return """{"blocked":false,"rule":null,"source":null}"""
                }

                override fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String> = error("not streamed")
            }
        },
    )

    @Test
    fun givenAnApp_whenARequestIsChecked_thenTheAppFollowsTheJsonFlag() = runTest {
        client.check("https://ads.example/a", "https://news.example/", "script", app = "12:com.example.news")

        assertEquals(listOf("check", "--json", "--app", "12:com.example.news", "https://ads.example/a", "https://news.example/", "script"), commands.single())
    }

    @Test
    fun givenNoApp_whenARequestIsChecked_thenNoAppIsNamed() = runTest {
        client.check("https://ads.example/a", page = null, kind = "image")

        assertEquals(listOf("check", "--json", "https://ads.example/a", "", "image"), commands.single())
    }

    @Test
    fun givenADomainOnlyEvent_whenChecked_thenTheHostUsesItsOwnCheckMode() = runTest {
        client.checkHost("ads.example", "12:com.example.news")

        assertEquals(listOf("check", "--json", "--app", "12:com.example.news", "--host", "ads.example"), commands.single())
    }

    @Test
    fun givenAnExactRule_whenRemovedAndRestored_thenBothCommandsSendItThroughStandardInput() = runTest {
        val rule = "@@||ads.example^\$app=com.Example.News\n"

        assertTrue(client.removeUserRules(rule))
        assertTrue(client.appendUniqueUserRule(rule))

        assertEquals(listOf(listOf("rules", "remove", "--json"), listOf("rules", "append", "--unique", "--json")), commands)
        assertEquals(listOf<String?>(rule, rule), inputs)
    }

    @Test
    fun givenAnOlderDaemon_whenAppendingAUniqueRule_thenThePlainAppendStillWorksWithoutAnUndoReceipt() = runTest {
        legacyDaemon = true

        assertFalse(client.appendUniqueUserRule("||ads.example^\n"))

        assertEquals(listOf(listOf("rules", "append", "--unique", "--json"), listOf("rules", "append", "--unique")), commands)
    }
}
