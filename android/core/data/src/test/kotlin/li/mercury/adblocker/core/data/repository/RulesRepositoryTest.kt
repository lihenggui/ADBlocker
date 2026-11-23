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
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.testing.unexpected
import java.io.ByteArrayInputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

class RulesRepositoryTest {
    private val result = RuleCheckResult(blocked = false, rule = null, source = null)
    private val checks = mutableListOf<Triple<String, String?, String?>>()
    private val rules = object : RulesRepository by unexpected() {
        override suspend fun checkUrl(url: String, page: String?, kind: String?): RuleCheckResult {
            checks += Triple(url, page, kind)
            return result
        }
    }

    @Test
    fun givenAnEnteredAddressAndBlankPage_whenChecked_thenHttpsIsUsedAndThePageIsUnknown() = runTest {
        val checked = rules.checkEnteredUrl("  ads.example/banner?q=1  ", "  ", "image")

        assertEquals(result, checked)
        assertEquals(Triple<String, String?, String?>("https://ads.example/banner?q=1", null, "image"), checks.single())
    }

    @Test
    fun givenAnEnteredAddressWithASchemeAndPage_whenChecked_thenTheirContextIsKept() = runTest {
        rules.checkEnteredUrl(" http://ads.example/banner ", " news.example ", "script")

        assertEquals(Triple<String, String?, String?>("http://ads.example/banner", "news.example", "script"), checks.single())
    }

    @Test
    fun aPastedAddressIsCutDownToItsHost() {
        assertEquals("ads.example.com", hostOf("https://user@Ads.Example.com:8443/path?q=1#top"))
        assertEquals("ads.example.com", hostOf("  ads.example.com.  "))
        assertEquals("cdn.example.co.uk", hostOf("cdn.example.co.uk/script.js"))
    }

    @Test
    fun whatIsNotAHostIsRefused() {
        assertNull(hostOf("localhost"))
        assertNull(hostOf(""))
        assertNull(hostOf("a..example"))
        assertNull(hostOf("ads_example.com"))
        assertNull(hostOf("a".repeat(64) + ".example"))
    }

    @Test
    fun aBlockRuleCoversTheDomainAndEveryNameBelowIt() {
        assertEquals("||ads.example.com^", blockRule(" .Ads.Example.com "))
    }

    @Test
    fun aFileUpToTheLimitIsReadWhole() {
        val bytes = ByteArray(1_000) { 'a'.code.toByte() }
        assertEquals(1_000, readLimited(ByteArrayInputStream(bytes), max = 1_000).size)
    }

    @Test
    fun aFileOverTheLimitIsRefusedWithoutBeingReadWhole() {
        var read = 0
        val endless = object : java.io.InputStream() {
            override fun read(): Int = 'a'.code.also { read++ }
        }
        assertFailsWith<RulesTooLargeException> { readLimited(endless, max = 1_000) }
        assertEquals(1_001, read)
    }
}
