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

import android.net.Uri
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdClient
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.testing.unexpected
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.ByteArrayInputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class RulesRepositoryTest {
    private val result = RuleCheckResult(blocked = false, rule = null, source = null)
    private val checks = mutableListOf<Triple<String, String?, String?>>()
    private val rules = object : RulesRepository by unexpected() {
        override suspend fun checkUrl(url: String, page: String?, kind: String?, app: String?): RuleCheckResult {
            assertNull(app)
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
    fun givenUserRules_whenExported_thenTheyGoToTheDocumentWriter() = runTest {
        val destination = Uri.parse("content://example.documents/rules")
        val writes = mutableListOf<Pair<Uri, String>>()
        val abd = object : AbdClient by unexpected() {
            override suspend fun userRules(): String = "||ads.example^\n"
        }
        val repository = AbdRulesRepository(
            RuntimeEnvironment.getApplication(),
            abd,
            DataChanges(),
            DocumentWriter { uri, bytes -> writes += uri to bytes.decodeToString() },
            StandardTestDispatcher(testScheduler),
        )

        repository.export(destination)

        assertEquals(listOf(destination to "||ads.example^\n"), writes)
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
    fun givenAnApp_whenABlockRuleIsWritten_thenItNamesThePackageInLowerCase() {
        assertEquals("||janapi.example^\$app=com.example.reader", blockRule("JanApi.example", app = "com.Example.Reader"))
    }

    @Test
    fun givenAnApp_whenADomainIsBlocked_thenItsRuleIsAppendedOnce() = runTest {
        val appended = mutableListOf<Pair<String, Boolean>>()
        val abd = object : AbdClient by unexpected() {
            override suspend fun appendUserRules(text: String, unique: Boolean) {
                appended += text to unique
            }
        }
        val repository = AbdRulesRepository(RuntimeEnvironment.getApplication(), abd, DataChanges(), DocumentWriter { _, _ -> }, StandardTestDispatcher(testScheduler))

        repository.blockDomain("ads.example", app = "com.example.reader")

        assertEquals(listOf("||ads.example^\$app=com.example.reader\n" to true), appended)
    }

    @Test
    fun givenAHostWithMixedCase_whenAllowed_thenCanonicalExceptionsAreAppendedOncePerScope() = runTest {
        val appended = mutableListOf<Pair<String, Boolean>>()
        val abd = object : AbdClient by unexpected() {
            override suspend fun appendUserRules(text: String, unique: Boolean) {
                appended += text to unique
            }
        }
        val repository = AbdRulesRepository(RuntimeEnvironment.getApplication(), abd, DataChanges(), DocumentWriter { _, _ -> }, StandardTestDispatcher(testScheduler))

        repository.allowDomain(" .Cdn.Example.com ")
        repository.allowDomain(" .Cdn.Example.com ", app = "com.Example.Reader")

        assertEquals(
            listOf("@@||cdn.example.com^\n" to true, "@@||cdn.example.com^\$app=com.example.reader\n" to true),
            appended,
        )
    }

    @Test
    fun givenAnOriginalRule_whenRemovedAndRestored_thenBothEditsUseItsExactLineAndNotify() = runTest {
        val rule = "@@||Ads.Example.com^\$app=com.Example.Reader"
        val edits = mutableListOf<Pair<String, String>>()
        val abd = object : AbdClient by unexpected() {
            override suspend fun removeUserRules(text: String): Boolean = true.also { edits += "remove" to text }
            override suspend fun appendUniqueUserRule(text: String): Boolean = true.also { edits += "append unique" to text }
        }
        val changes = DataChanges()
        val repository = AbdRulesRepository(RuntimeEnvironment.getApplication(), abd, changes, DocumentWriter { _, _ -> }, StandardTestDispatcher(testScheduler))
        val removed = async(start = CoroutineStart.UNDISPATCHED) { changes.changes.first() }
        assertTrue(repository.removeRule(rule))
        removed.await()
        val restored = async(start = CoroutineStart.UNDISPATCHED) { changes.changes.first() }
        assertTrue(repository.addRule(rule))
        restored.await()

        assertEquals(listOf("remove" to "$rule\n", "append unique" to "$rule\n"), edits)
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
