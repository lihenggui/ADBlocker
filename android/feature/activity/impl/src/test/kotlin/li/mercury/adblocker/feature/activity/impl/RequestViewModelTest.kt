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

package li.mercury.adblocker.feature.activity.impl

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.repository.ActivityBuffer
import li.mercury.adblocker.core.data.repository.ActivityFeed
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.data.repository.allowRule
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class RequestViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    @Test
    fun givenAHostWasBlocked_whenTheClosingSheetIsCollectedAgain_thenTheConfirmationIsStillPending() = runTest {
        val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Allowed, "ads.example", null, null, null, null)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = null
        }
        val blocked = mutableListOf<Pair<String, String?>>()
        val rules = object : RulesRepository by unexpected() {
            override suspend fun blockDomain(domain: String, app: String?) {
                blocked += domain to app
            }
        }
        val viewModel = RequestViewModel(request.id, recorder, apps, rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.blockHost()
        runCurrent()
        assertEquals(listOf<Pair<String, String?>>(request.host to null), blocked)
        assertTrue(viewModel.uiState.value.done)

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_activity_blocked_host_everywhere, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)
    }

    @Test
    fun givenAPersonalApp_whenTheHostIsBlockedInIt_thenTheRuleNamesItsPackageAndTheMessageItsLabel() = runTest {
        val request = allowed("com.example.news")
        val blocked = mutableListOf<Pair<String, String?>>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), blocking(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals("com.example.news", viewModel.uiState.value.rulePackage)

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(listOf<Pair<String, String?>>("ads.example" to "com.example.news"), blocked)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_blocked_host_in_app, message.text)
        assertEquals(listOf<Any>("\u2066ads.example\u2069", "News"), message.args)
    }

    @Test
    fun givenAWorkProfileApp_whenTheHostIsBlockedInIt_thenTheRuleNamesThePackageWithoutTheProfile() = runTest {
        val request = allowed("com.example.news", profileSerial = 12)
        val blocked = mutableListOf<Pair<String, String?>>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), blocking(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(listOf<Pair<String, String?>>("ads.example" to "com.example.news"), blocked)
        assertEquals(listOf<Any>("\u2066ads.example\u2069", "\u2066com.example.news\u2069"), viewModel.pendingMessages.value.single().message.args)
    }

    @Test
    fun givenASharedUidGroup_whenOpened_thenTheHostCanOnlyBeBlockedInEveryApp() = runTest {
        val request = allowed("@com.example.shared")
        val blocked = mutableListOf<Pair<String, String?>>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), blocking(blocked))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertNull(viewModel.uiState.value.rulePackage)

        viewModel.blockHostInApp()
        runCurrent()

        assertEquals(emptyList(), blocked)
        assertFalse(viewModel.uiState.value.done)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun givenABlockedEndpointOrIpv6Address_whenAllowActionsAreInvoked_thenNoExceptionIsOfferedOrWritten() = runTest {
        for (endpoint in listOf("1.2.3.4:443", "[2001:db8::1]:443", "2001:db8::1", "[2001:db8::1]")) {
            val request = allowed("com.example.news").copy(host = endpoint, verdict = Verdict.HostBlocked)
            val appended = mutableListOf<String>()
            val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), allowing(appended))

            assertNull(viewModel.uiState.value.allowHost, endpoint)
            viewModel.allowHost()
            viewModel.allowHostInApp()
            runCurrent()

            assertTrue(appended.isEmpty(), endpoint)
            assertTrue(viewModel.pendingMessages.value.isEmpty(), endpoint)
        }
    }

    @Test
    fun givenABlockedNameOrIpv4Address_whenOpened_thenItCanBeAllowed() {
        for ((host, ruleHost) in listOf(
            "cdn.example.com" to "cdn.example.com",
            "CDN.Example.com." to "cdn.example.com",
            "1.2.3.4" to "1.2.3.4",
        )) {
            val request = allowed("com.example.news").copy(host = host, verdict = Verdict.HostBlocked)
            assertEquals(ruleHost, RequestUiState(request = request).allowHost, host)
        }
    }

    @Test
    fun givenAHostWasRefused_whenAllowedInAllApps_thenTheExceptionNamesTheRecordedHost() = runTest {
        val request = allowed("com.example.news").copy(host = "cdn.example.com", verdict = Verdict.HostBlocked)
        val appended = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), allowing(appended))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals("example.com", viewModel.uiState.value.trustSite)
        assertNull(viewModel.uiState.value.rule)

        viewModel.allowHost()
        runCurrent()

        assertEquals(listOf("@@||cdn.example.com^"), appended)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_allowed_host_everywhere, message.text)
        assertEquals(listOf<Any>("\u2066cdn.example.com\u2069"), message.args)
    }

    @Test
    fun givenAWorkAppAndBlockedHost_whenAllowedInThatApp_thenTheExceptionNamesItsPackageAndTheMessageItsLabel() = runTest {
        val request = allowed("com.example.news", profileSerial = 12).copy(host = "cdn.example.com", verdict = Verdict.HostBlocked)
        val appended = mutableListOf<String>()
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, "News"), allowing(appended))
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.allowHostInApp()
        runCurrent()

        assertEquals(listOf("@@||cdn.example.com^\$app=com.example.news"), appended)
        assertTrue(viewModel.uiState.value.done)
        val message = viewModel.pendingMessages.value.single().message
        assertEquals(R.string.feature_activity_allowed_host_in_app, message.text)
        assertEquals(listOf<Any>("\u2066cdn.example.com\u2069", "News"), message.args)
    }

    @Test
    fun givenAnAllowedRequest_whenTrusted_thenItsSiteIsAddedToTrustedSites() = runTest {
        val request = allowed("com.example.news").copy(host = "cdn.example.com")
        val trusted = mutableListOf<String>()
        val rules = object : RulesRepository by unexpected() {
            override suspend fun trust(host: String) {
                trusted += host
            }
        }
        val viewModel = RequestViewModel(request.id, recorderOf(request), appsNaming(request, null), rules)
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()

        viewModel.trustSite()
        runCurrent()

        assertEquals(listOf("example.com"), trusted)
        assertTrue(viewModel.uiState.value.done)
        assertEquals(R.string.feature_activity_trusted, viewModel.pendingMessages.value.single().message.text)
    }

    private fun allowed(packageName: String, profileSerial: Long? = null) = RequestEvent(1, Instant.EPOCH, packageName, Verdict.Allowed, "ads.example", null, null, null, null, profileSerial)

    private suspend fun recorderOf(request: RequestEvent): ActivityRecorder {
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        return recorder
    }

    private fun appsNaming(request: RequestEvent, label: String?) = object : AppsRepository by unexpected() {
        override suspend fun app(packageName: String) = label?.let {
            assertEquals(request.appKey, packageName)
            AppEntry(InstalledApp(request.packageName, it, icon = null, isSystem = false), filter = null)
        }
    }

    private fun blocking(blocked: MutableList<Pair<String, String?>>) = object : RulesRepository by unexpected() {
        override suspend fun blockDomain(domain: String, app: String?) {
            blocked += domain to app
        }
    }

    private fun allowing(appended: MutableList<String>) = object : RulesRepository by unexpected() {
        override suspend fun allowDomain(domain: String, app: String?) {
            appended += allowRule(domain, app)
        }
    }

    @Test
    fun givenARecordingWasReplaced_whenAnOldRequestIsOpened_thenItIsGoneAndOffersNoActions() = runTest {
        val previous = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Allowed, "ads.example", null, null, null, null)
        val current = previous.copy(id = 2, host = "news.example")
        var feed = ActivityFeed(listOf(previous), 0)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(feed)
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        assertEquals(listOf(previous), recorder.state.value.feed.events)
        feed = ActivityFeed(listOf(current), 0)
        assertTrue(recorder.requestStart())
        recorder.record()
        assertEquals(listOf(current), recorder.state.value.feed.events)

        val viewModel = RequestViewModel(previous.id, recorder, unexpected(), unexpected())
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        viewModel.blockHost()
        viewModel.trustSite()
        viewModel.forgetPinned()
        runCurrent()

        val state = viewModel.uiState.value
        assertTrue(state.gone)
        assertNull(state.request)
        assertNull(state.trustSite)
        assertFalse(state.done)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun givenARecordedUrl_whenLookupFails_thenTheRequestRemainsWithAnUnavailableRuleMatch() = runTest {
        val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Blocked, "ads.example", "https://ads.example/banner", null, null, null)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        var appReads = 0
        val apps = object : AppsRepository by unexpected() {
            override suspend fun app(packageName: String) = null.also { appReads++ }
        }
        var fail = true
        val rules = object : RulesRepository by unexpected() {
            override suspend fun checkUrl(url: String, page: String?, kind: String?, app: String?): RuleCheckResult {
                assertEquals(request.url, url)
                assertEquals("com.example.news", app)
                assertNull(page)
                assertNull(kind)
                if (fail) throw AbdException.TimedOut()
                return RuleCheckResult(blocked = false, rule = null, source = null)
            }
        }
        val viewModel = RequestViewModel(request.id, recorder, apps, rules)
        runCurrent()
        assertEquals(0, appReads)
        val shown = backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        val unavailable = viewModel.uiState.value
        assertEquals(request, unavailable.request)
        assertFalse(unavailable.gone)
        assertTrue(unavailable.ruleCheckUnavailable)
        assertNull(unavailable.rule)

        shown.cancel()
        advanceTimeBy(5_001)
        assertEquals(1, appReads)
        fail = false
        backgroundScope.launch { viewModel.uiState.collect {} }
        runCurrent()
        assertEquals(2, appReads)
        assertFalse(viewModel.uiState.value.ruleCheckUnavailable)
        assertEquals(false, viewModel.uiState.value.rule?.blocked)
    }
}
