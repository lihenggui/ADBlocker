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

package li.mercury.adblocker.feature.overview.impl

import android.content.Intent
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.repository.OverviewRepository
import li.mercury.adblocker.core.data.repository.OverviewSnapshot
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.AppEntry
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.StatsSummary
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

@OptIn(ExperimentalCoroutinesApi::class)
class OverviewViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    /** What was done, in order: the order is what the tests are about. */
    private val done = mutableListOf<String>()

    private var root = true
    private var rootAsked = 0
    private var snapshotsRead = 0
    private var appsRead = 0
    private var appsAfter: suspend () -> List<AppEntry> = { emptyList() }
    private var filteringApps: Int? = null
    private val changes = DataChanges()
    private val userData = TestUserDataRepository(UserData(filterMethod = FilterMethod.Vpn))
    private val vpnStatus = MutableStateFlow(VpnStatus(state = VpnState.On))

    private val vpn = object : VpnController {
        override suspend fun engineAvailable() = true
        override val status: StateFlow<VpnStatus> = vpnStatus

        override fun consentIntent(): Intent? = null

        override suspend fun start() {
            done += "vpn start"
        }

        override suspend fun stop() {
            done += "vpn stop"
        }

        override suspend fun rebuild() = Unit
    }

    private val methods = object : FilterMethodRepository by unexpected() {
        override suspend fun rootAvailable(): Boolean {
            rootAsked++
            return root
        }

        override suspend fun switchTo(method: FilterMethod): FilterMethodSwitch {
            done += "method $method"
            userData.setFilterMethod(method)
            return FilterMethodSwitch.Complete
        }
    }

    private val overview = object : OverviewRepository {
        private val status = DaemonStatus(
            running = true, pid = null, startedAt = null, version = null, apps = 0, rules = 1, hostRules = null,
            exclusions = null, paused = false, resumeAt = null, ipv6 = null, listsUpdated = null, urlFiltering = false,
        )

        override suspend fun snapshot(): OverviewSnapshot {
            snapshotsRead++
            val current = filteringApps?.let { status.copy(apps = 5, filteringApps = it) } ?: status
            return OverviewSnapshot(current, StatsSummary.Empty, emptyList())
        }
    }

    private val viewModel by lazy {
        OverviewViewModel(
            daemon = unexpected<DaemonRepository>(),
            apps = object : AppsRepository by unexpected() {
                override suspend fun apps(): List<AppEntry> {
                    appsRead++
                    return appsAfter()
                }
            },
            overview = overview,
            module = object : ModuleRepository by unexpected() {},
            userData = userData,
            vpn = vpn,
            methods = methods,
            https = object : HttpsRepository by unexpected() {
                override fun certificateInstalled() = false
            },
            io = main.testDispatcher,
            defaultDispatcher = main.testDispatcher,
            changes = changes,
        )
    }

    private fun TestScope.shown(): OverviewUiState.Ready {
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { viewModel.uiState.collect {} }
        return viewModel.uiState.value as OverviewUiState.Ready
    }

    @Test
    fun givenVpnConsentIsRefused_whenTheOutcomeIsCollectedAgain_thenItWaitsForAcknowledgement() = runTest {
        viewModel.consentRefused()

        val message = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_overview_vpn_refused, message.message.text)
        assertEquals(message, viewModel.pendingMessages.first().single())
        viewModel.onMessageShown(message.id)
        assertEquals(emptyList(), viewModel.pendingMessages.value)
    }

    @Test
    fun givenVpnMode_whenStateIsCollectedWithoutStartingTheScreen_thenRootIsNotRequested() = runTest {
        shown()
        assertEquals(0, rootAsked)
        viewModel.onStart()
        assertEquals(OverviewNotice.RootAvailable, (viewModel.uiState.value as OverviewUiState.Ready).notice)
        viewModel.onStart()
        assertEquals(1, rootAsked)
    }

    @Test
    fun rootGrantedSinceTheVpnWasChosenIsOfferedUntilDismissed() = runTest {
        viewModel.onStart()
        assertEquals(OverviewNotice.RootAvailable, shown().notice)
        viewModel.dismissNotice(OverviewNotice.RootAvailable)
        assertNull((viewModel.uiState.value as OverviewUiState.Ready).notice)
        assertEquals(1, rootAsked)
    }

    @Test
    fun lockdownIsNoticedBeforeRoot() = runTest {
        vpnStatus.value = VpnStatus(state = VpnState.On, alwaysOn = true, lockdown = true)
        viewModel.onStart()
        assertEquals(OverviewNotice.Lockdown, shown().notice)
    }

    @Test
    fun underRootNothingAsksForRootNorNoticesTheVpn() = runTest {
        userData.setFilterMethod(FilterMethod.Root)
        vpnStatus.value = VpnStatus(lockdown = true)
        viewModel.onStart()
        assertNull(shown().notice)
        assertEquals(0, rootAsked)
    }

    @Test
    fun givenRootStatusWithDifferentConfiguredAndArmedCounts_whenShown_thenOverviewUsesArmedCount() = runTest {
        userData.setFilterMethod(FilterMethod.Root)
        filteringApps = 3
        val state = shown()
        assertEquals(5, state.status.apps)
        assertEquals(3, state.filteredApps)
    }

    @Test
    fun switchingToRootDelegatesTheHandoffToTheMethodRepository() = runTest {
        shown()
        viewModel.switchToRoot()
        assertEquals(listOf("method Root"), done)
        assertEquals(FilterMethod.Root, userData.userData.value.method)
    }

    @Test
    fun aDisconnectedVpnIsShownAsTheVpnHasIt() = runTest {
        shown()
        vpnStatus.value = VpnStatus(state = VpnState.Revoked)
        assertEquals(VpnState.Revoked, (viewModel.uiState.value as OverviewUiState.Ready).vpn.state)
    }

    @Test
    fun ticksAndVpnStatusChangesReuseTheAppsUntilARefreshOrDataChange() = runTest {
        root = false
        shown()
        assertEquals(1, snapshotsRead)
        assertEquals(1, appsRead)

        advanceTimeBy(30_000)
        runCurrent()
        vpnStatus.value = VpnStatus(state = VpnState.Revoked)
        assertEquals(3, snapshotsRead)
        assertEquals(1, appsRead)

        viewModel.refresh()
        changes.notifyChanged()
        assertEquals(5, snapshotsRead)
        assertEquals(3, appsRead)
    }

    @Test
    fun givenATickArrivesWhileAChangeReloadsTheApps_whenTheReplacementLoadRuns_thenItReadsTheApps() = runTest {
        root = false
        shown()
        val news = InstalledApp("com.example.news", "News", null, isSystem = false)
        val gate = CompletableDeferred<Unit>()
        appsAfter = {
            gate.await()
            listOf(AppEntry(news, FilteredApp(news.packageName, FilterLevel.Hosts, pausedUntil = null)))
        }

        changes.notifyChanged()
        assertEquals(2, appsRead)
        vpnStatus.value = VpnStatus(state = VpnState.Revoked)
        gate.complete(Unit)
        runCurrent()

        assertEquals(1, (viewModel.uiState.value as OverviewUiState.Ready).filteredApps)
    }
}
