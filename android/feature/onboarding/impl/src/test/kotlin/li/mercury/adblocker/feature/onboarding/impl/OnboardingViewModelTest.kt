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

package li.mercury.adblocker.feature.onboarding.impl

import android.content.Intent
import android.os.UserHandle
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DeviceRepository
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.ListUpdateScheduler
import li.mercury.adblocker.core.data.repository.ListsRepository
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.AppIconSource
import li.mercury.adblocker.core.model.AppInventory
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.DeviceCheck
import li.mercury.adblocker.core.model.DeviceCheckKind
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.InstalledApp
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.SharedAppMember
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.model.WorkApp
import li.mercury.adblocker.core.model.WorkSelection
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@OptIn(ExperimentalCoroutinesApi::class)
class OnboardingViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    /** What was done, in order, by every double: the order is what the tests are about. */
    private val done = mutableListOf<String>()

    private var root = false
    private var region: String? = "CN"
    private var switchResult = FilterMethodSwitch.Complete
    private var vpnFails = false
    private var vpnAfterStart = VpnState.On
    private val vpnStatus = MutableStateFlow(VpnStatus())
    private var suspendDownload = false
    private var downloadCancelled = false
    private val userData = TestUserDataRepository()
    private val subscriptions = FilterCatalog.setupLists("CN").toMutableMap()
    private val browser = InstalledApp("com.example.browser", "Browser", AppIconSource("com.example.browser", "test"), isSystem = false)
    private val ordinaryApp = InstalledApp("com.example.notes", "Notes", AppIconSource("com.example.notes", "test"), isSystem = false)

    private val device = object : DeviceRepository {
        override fun regionCode() = region

        override suspend fun check(kind: DeviceCheckKind) = DeviceCheck(
            kind,
            passed = when (kind) {
                DeviceCheckKind.Root, DeviceCheckKind.Kernel -> root
                else -> true
            },
            detail = null,
        )
    }

    private var configuredApps = emptySet<String>()
    private var configuredScope: FilteringScope? = null
    private var workApp: InstalledApp? = null
    private var workUnavailable = false
    private var personalUnavailable = false
    private var sharedUid = false

    private val apps = object : AppsRepository by unexpected() {
        override suspend fun installed(method: FilterMethod) = AppInventory(
            listOf(
                ordinaryApp.copy(unavailableReason = AppUnavailableReason.SharedUid.takeIf { sharedUid && method == FilterMethod.Root }),
                browser,
            ) + listOfNotNull(workApp.takeIf { method == FilterMethod.Root }),
            workUnavailable = workUnavailable && method == FilterMethod.Root,
            personalUnavailable = personalUnavailable && method == FilterMethod.Root,
        )

        override suspend fun configure(packages: Set<String>, work: WorkSelection) {
            configuredApps = packages
            done += "configure apps"
        }
    }

    private val lists = object : ListsRepository by unexpected() {
        override suspend fun configure(choices: Map<String, Boolean>) {
            subscriptions.putAll(choices)
            done += "configure lists"
        }

        override fun update(): Flow<UpdateEvent> = flow {
            done += "update"
            emit(UpdateEvent.Started(1))
            if (suspendDownload) {
                try {
                    awaitCancellation()
                } finally {
                    downloadCancelled = true
                }
            }
            emit(UpdateEvent.Finished(updated = 1, unchanged = 0, failed = 0))
        }
    }

    private val methods = object : FilterMethodRepository by unexpected() {
        override suspend fun rootAvailable() = root

        override suspend fun switchTo(method: FilterMethod): FilterMethodSwitch {
            done += "method $method"
            return switchResult
        }
    }

    private val vpn = object : VpnController {
        override suspend fun engineAvailable() = true
        override val status: StateFlow<VpnStatus> = vpnStatus

        override fun consentIntent(): Intent? = null

        override suspend fun start() {
            if (vpnFails) throw AbdException.EngineMissing()
            done += "vpn start"
            vpnStatus.value = VpnStatus(state = vpnAfterStart)
        }

        override suspend fun stop() = Unit
    }

    private val daemon = object : DaemonRepository by unexpected() {
        override suspend fun setFilteringScope(scope: FilteringScope) {
            configuredScope = scope
        }

        override suspend fun status() = DaemonStatus(
            running = true, pid = null, startedAt = null, version = null, apps = 1, rules = 1_000, hostRules = null,
            exclusions = null, paused = false, resumeAt = null, ipv6 = null, listsUpdated = null,
        )
    }

    private val viewModel by lazy { newViewModel() }

    private fun newViewModel() = OnboardingViewModel(
        device = device,
        module = object : ModuleRepository by unexpected() {
            override suspend fun state() = ModuleState.NeedsRestart

            override suspend fun restartDevice() {
                assertTrue(userData.userData.value.onboardingDone)
                done += "restart"
            }
        },
        apps = apps,
        lists = lists,
        userData = userData,
        scheduler = object : ListUpdateScheduler by unexpected() {
            override suspend fun updateSoon() = Unit

            override suspend fun updateRootSoon() {
                assertTrue(userData.userData.value.onboardingDone)
                done += "schedule root update"
            }
        },
        methods = methods,
        vpn = vpn,
        daemon = daemon,
    )

    /** Through the check, to the step after it. */
    private fun leaveTheCheck(model: OnboardingViewModel = viewModel) {
        model.next()
        model.next()
    }

    private fun pickBrowser() {
        viewModel.next()
        viewModel.togglePick(browser.appKey)
    }

    @Test
    fun appsAreListedForManualSelectionAndBackNavigationKeepsTheChoice() = runTest {
        leaveTheCheck()
        viewModel.next()
        assertEquals(listOf(browser, ordinaryApp), viewModel.uiState.value.apps)
        assertTrue(viewModel.uiState.value.picks.isEmpty())
        viewModel.togglePick(ordinaryApp.packageName)
        viewModel.next()
        viewModel.back()
        assertEquals(setOf(ordinaryApp.packageName), viewModel.uiState.value.picks)
    }

    @Test
    fun givenARootGroup_whenChoosingApps_thenOnlyAnExplicitPickSelectsOneGroupKey() = runTest {
        root = true
        val group = ordinaryApp.copy(
            packageName = "@com.example.suite",
            sharedProfileSerial = 37,
            sharedMembers = listOf(SharedAppMember(ordinaryApp.packageName, ordinaryApp.label), SharedAppMember("com.example.hidden", "com.example.hidden")),
        )
        workApp = group
        leaveTheCheck()
        viewModel.next()
        assertTrue(viewModel.uiState.value.picks.isEmpty())
        viewModel.togglePick(group.appKey)
        assertEquals(setOf("37:@com.example.suite"), viewModel.uiState.value.picks)
        viewModel.togglePick(group.appKey)
        assertTrue(viewModel.uiState.value.picks.isEmpty())
    }

    @Test
    fun givenTheSamePackageInBothProfiles_whenSettingUpRoot_thenBothInstancesCanBeChosen() = runTest {
        root = true
        val work = InstalledApp(
            packageName = browser.packageName,
            label = browser.label,
            icon = null,
            isSystem = false,
            work = WorkApp(37, UserHandle.getUserHandleForUid(1_010_000), null, quiet = false, available = true),
        )
        workApp = work
        leaveTheCheck()
        viewModel.next()
        assertEquals(setOf(browser.appKey, work.appKey), viewModel.uiState.value.apps.filter { it.packageName == browser.packageName }.map { it.appKey }.toSet())

        viewModel.togglePick(work.appKey)
        assertEquals(setOf(work.appKey), viewModel.uiState.value.picks)
        viewModel.togglePick(browser.appKey)
        assertEquals(setOf(browser.appKey, work.appKey), viewModel.uiState.value.picks)

        viewModel.back()
        viewModel.back()
        root = false
        viewModel.runChecks()
        runCurrent()
        viewModel.next()
        viewModel.next()
        assertEquals(FilterMethod.Vpn, viewModel.uiState.value.method)
        assertTrue(viewModel.uiState.value.apps.none { it.work != null })
        assertEquals(setOf(browser.appKey), viewModel.uiState.value.picks)

        viewModel.back()
        viewModel.back()
        root = true
        viewModel.runChecks()
        runCurrent()
        viewModel.next()
        viewModel.next()
        assertEquals(setOf(browser.appKey, work.appKey), viewModel.uiState.value.picks)
        viewModel.next()
        viewModel.next()
        assertEquals(setOf(browser.appKey, work.appKey), userData.userData.value.pendingApps)
        assertFalse(userData.userData.value.pendingPreserveWorkApps)
    }

    @Test
    fun givenPersonalEligibilityFailure_whenContinuingSetup_thenRetryIsRequiredAndChoiceSurvives() = runTest {
        root = true
        personalUnavailable = true
        leaveTheCheck()
        pickBrowser()
        assertEquals(setOf(browser.appKey), viewModel.uiState.value.picks)
        assertTrue(viewModel.uiState.value.personalEligibilityUnavailable)

        viewModel.next()
        assertEquals(OnboardingStep.Apps, viewModel.uiState.value.step)

        personalUnavailable = false
        sharedUid = true
        viewModel.loadApps()
        viewModel.togglePick(ordinaryApp.appKey)
        assertFalse(viewModel.uiState.value.personalEligibilityUnavailable)
        assertEquals(setOf(browser.appKey), viewModel.uiState.value.picks)

        viewModel.next()
        assertEquals(OnboardingStep.Lists, viewModel.uiState.value.step)
    }

    @Test
    fun givenAllAppsWithNoExplicitPicks_whenInstallingVpn_thenVpnStarts() = runTest {
        val vm = viewModel
        assertFalse(vm.uiState.value.willStartVpn)
        vm.setFilteringScope(FilteringScope.All)
        assertTrue(vm.uiState.value.willStartVpn)
        vm.startVpn()
        assertEquals(FilteringScope.All, configuredScope)
        assertTrue("vpn start" in done)
    }

    @Test
    fun givenSharedUidUncheckedByRoot_whenSettingUpVpn_thenPersonalChoiceIsAvailable() = runTest {
        sharedUid = true
        leaveTheCheck()
        viewModel.next()
        viewModel.togglePick(ordinaryApp.appKey)
        assertEquals(FilterMethod.Vpn, viewModel.uiState.value.method)
        assertEquals(setOf(ordinaryApp.appKey), viewModel.uiState.value.picks)
    }

    @Test
    fun givenIncompleteRootInventory_whenSetupIsDeferred_thenWorkPreservationIsRemembered() = runTest {
        root = true
        workUnavailable = true
        leaveTheCheck()
        pickBrowser()
        assertEquals(setOf(browser.appKey), viewModel.uiState.value.picks)

        viewModel.next()
        viewModel.next()
        assertEquals(setOf(browser.appKey), userData.userData.value.pendingApps)
        assertTrue(userData.userData.value.pendingPreserveWorkApps)
    }

    @Test
    fun onlyTheMatchingRegionalListIsSelected() {
        val china = FilterCatalog.entries.first { it.id == "easylist_china" }.url
        val japan = FilterCatalog.entries.first { it.id == "japanese_filters" }.url

        assertEquals(true, FilterCatalog.setupLists("CN")[china])
        assertEquals(null, FilterCatalog.setupLists("CN")[japan])
        assertEquals(false, FilterCatalog.setupLists("JP")[china])
        assertEquals(true, FilterCatalog.setupLists("JP")[japan])
        assertEquals(false, FilterCatalog.setupLists("US")[china])
        assertEquals(FilterCatalog.setupLists("US"), FilterCatalog.setupLists(null))
    }

    @Test
    fun deferredRootSetupRetainsTheChosenRegionalList() = runTest {
        root = true
        region = "JP"
        leaveTheCheck()
        repeat(3) { viewModel.next() }

        val china = FilterCatalog.entries.first { it.id == "easylist_china" }.url
        val japan = FilterCatalog.entries.first { it.id == "japanese_filters" }.url
        val saved = userData.userData.value
        assertEquals(setOf(china), saved.pendingDisabledLists)
        assertTrue(japan in saved.pendingEnabledLists)
        assertTrue(saved.pendingApps.isEmpty())

        viewModel.restartDevice()
        assertEquals(listOf("schedule root update", "restart"), done)
    }

    @Test
    fun setupWithNoAppsSavesTheListsWithoutStartingTheVpn() = runTest {
        region = "JP"
        leaveTheCheck()
        repeat(3) { viewModel.next() }
        viewModel.startVpn()
        viewModel.finish()

        val china = FilterCatalog.entries.first { it.id == "easylist_china" }.url
        val japan = FilterCatalog.entries.first { it.id == "japanese_filters" }.url
        assertEquals(listOf("method Vpn", "configure lists", "configure apps"), done)
        assertEquals(false, subscriptions[china])
        assertEquals(true, subscriptions[japan])
        assertTrue(userData.userData.value.onboardingDone)
    }

    @Test
    fun withoutRootTheCheckGoesOnWithTheVpnAndNothingOfTheModuleIsTouched() = runTest {
        leaveTheCheck()
        assertEquals(OnboardingStep.About, viewModel.uiState.value.step)
        assertEquals(FilterMethod.Vpn, viewModel.uiState.value.method)

        // On to the start step: the VPN waits for its grants, and the module is never asked about.
        repeat(3) { viewModel.next() }
        assertEquals(OnboardingStep.Install, viewModel.uiState.value.step)
        assertEquals(InstallState.Idle, viewModel.uiState.value.install)
        assertTrue(done.isEmpty())
    }

    @Test
    fun withRootTheCheckGoesOnWithTheModule() = runTest {
        root = true
        leaveTheCheck()
        assertEquals(FilterMethod.Root, viewModel.uiState.value.method)
    }

    @Test
    fun theMethodIsStoredBeforeTheAppsAndListsAreWrittenAndTheVpnStartsBeforeTheDownload() = runTest {
        leaveTheCheck()
        pickBrowser()
        viewModel.toggleList(viewModel.uiState.value.lists.keys.first())
        viewModel.startVpn()

        assertEquals(listOf("method Vpn", "configure lists", "configure apps", "vpn start", "update"), done)
        val install = viewModel.uiState.value.install as InstallState.Done
        assertTrue(install.active)
        assertEquals(1_000, install.rules)
        assertFalse(install.listsFailed)
    }

    @Test
    fun givenRootSettingsCouldNotBeCarried_whenVpnSetupFinishes_thenTheIncompleteTransferRemainsVisible() = runTest {
        switchResult = FilterMethodSwitch.VpnTransferIncomplete
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()

        assertTrue(viewModel.uiState.value.vpnTransferIncomplete)
        assertTrue((viewModel.uiState.value.install as InstallState.Done).active)
        assertEquals(setOf(browser.packageName), configuredApps)
    }

    @Test
    fun aRefusedConsentGoesBackToTheStartStepSayingSoUntilItIsGiven() = runTest {
        leaveTheCheck()
        viewModel.consentRefused()
        assertEquals(InstallState.Idle, viewModel.uiState.value.install)
        assertTrue(viewModel.uiState.value.consentRefused)
        assertTrue(done.isEmpty())

        pickBrowser()
        viewModel.startVpn()
        assertFalse(viewModel.uiState.value.consentRefused)
    }

    @Test
    fun aVpnThatCannotStartFailsWithoutDownloadingAndCanBeTriedAgain() = runTest {
        vpnFails = true
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()
        assertEquals(InstallState.Failed(LoadFailure.EngineMissing), viewModel.uiState.value.install)
        assertFalse("update" in done)

        vpnFails = false
        viewModel.startVpn()
        assertTrue(viewModel.uiState.value.install is InstallState.Done)
    }

    @Test
    fun retryingSetupInANewViewModelReplacesThePriorAppSelection() = runTest {
        vpnFails = true
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()

        assertEquals(setOf(browser.packageName), configuredApps)

        val restarted = newViewModel()
        leaveTheCheck(restarted)
        restarted.startVpn()
        assertEquals(emptySet(), configuredApps)
        assertEquals(InstallState.Done(active = false, apps = 0), restarted.uiState.value.install)
    }

    @Test
    fun theListsWaitForTheVpnAndStartupFailureCanBeRetried() = runTest {
        vpnAfterStart = VpnState.Starting
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()
        assertEquals(InstallState.Running(InstallStage.Vpn, 0.3f), viewModel.uiState.value.install)
        assertFalse("update" in done)

        vpnStatus.value = VpnStatus(state = VpnState.Failed)
        runCurrent()
        assertEquals(InstallState.Failed(LoadFailure.Failed), viewModel.uiState.value.install)
        assertFalse("update" in done)

        vpnAfterStart = VpnState.On
        viewModel.startVpn()
        assertTrue(viewModel.uiState.value.install is InstallState.Done)
    }

    @Test
    fun losingTheVpnDuringTheDownloadCancelsSetup() = runTest {
        vpnAfterStart = VpnState.Starting
        suspendDownload = true
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()
        vpnStatus.value = VpnStatus(state = VpnState.On)
        runCurrent()
        assertEquals(InstallState.Running(InstallStage.Lists, 0.4f), viewModel.uiState.value.install)

        vpnStatus.value = VpnStatus(state = VpnState.Revoked)
        runCurrent()
        assertTrue(downloadCancelled)
        assertEquals(InstallState.Failed(LoadFailure.Failed), viewModel.uiState.value.install)
    }

    @Test
    fun losingTheVpnBeforeFinishLeavesOnboardingOpen() = runTest {
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()
        assertTrue(viewModel.uiState.value.install is InstallState.Done)

        vpnStatus.value = VpnStatus(state = VpnState.Revoked)
        viewModel.finish()
        runCurrent()
        assertEquals(InstallState.Failed(LoadFailure.Failed), viewModel.uiState.value.install)
        assertFalse(userData.userData.value.onboardingDone)
    }

    @Test
    fun startupTimesOutIfTheServiceNeverAnswers() = runTest {
        vpnAfterStart = VpnState.Starting
        leaveTheCheck()
        pickBrowser()
        viewModel.startVpn()
        advanceTimeBy(10_001L)
        assertEquals(InstallState.Failed(LoadFailure.TimedOut), viewModel.uiState.value.install)
        assertFalse("update" in done)
    }
}
