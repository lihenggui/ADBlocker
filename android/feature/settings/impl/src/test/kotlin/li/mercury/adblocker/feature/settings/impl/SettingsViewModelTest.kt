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

package li.mercury.adblocker.feature.settings.impl

import android.content.Intent
import android.net.Uri
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
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.apps.AppPlatformInfo
import li.mercury.adblocker.core.data.repository.AppsRepository
import li.mercury.adblocker.core.data.repository.DaemonRepository
import li.mercury.adblocker.core.data.repository.DataChanges
import li.mercury.adblocker.core.data.repository.DiagnosticsRepository
import li.mercury.adblocker.core.data.repository.FilterMethodRepository
import li.mercury.adblocker.core.data.repository.FilterMethodSwitch
import li.mercury.adblocker.core.data.repository.HttpsRepository
import li.mercury.adblocker.core.data.repository.LogExportResult
import li.mercury.adblocker.core.data.repository.ModuleRepository
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.testing.MainDispatcherRule
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue

// Under Robolectric for the document URI used to export logs.
@RunWith(RobolectricTestRunner::class)
@OptIn(ExperimentalCoroutinesApi::class)
class SettingsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    /** What was done, in order: the order is what the tests are about. */
    private val done = mutableListOf<String>()
    private var pinned = emptyList<PinnedHost>()
    private var pinsFailure: AbdException? = null
    private var logExport = LogExportResult.Complete
    private val forgotten = mutableListOf<Pair<String, String?>>()
    private val labelRequests = mutableListOf<Set<String>>()
    private var notificationsAllowed = true

    private val platform = object : AppPlatformInfo {
        override fun versionName() = "test version"

        override fun notificationsAllowed() = notificationsAllowed

        override suspend fun personalAppLabels(packages: Set<String>): Map<String, String> {
            labelRequests += packages
            return packages.associateWith { if (it == "com.example.news") "News" else it }
        }
    }

    private var rootAsked = 0
    private var statusReads = 0
    private var statusFailure: AbdException? = null
    private var urlFiltering = true
    private var cosmetic = true
    private var httpsWrite: CompletableDeferred<Unit>? = null
    private val changes = DataChanges()
    private val userData = TestUserDataRepository(UserData(filterMethod = FilterMethod.Vpn))

    private val vpn = object : VpnController {
        override suspend fun engineAvailable() = true
        override val status: StateFlow<VpnStatus> = MutableStateFlow(VpnStatus())

        override fun consentIntent(): Intent? = null

        override suspend fun start() = Unit

        override suspend fun stop() {
            done += "vpn stop"
        }

        override suspend fun rebuild() = Unit
    }

    private var moduleReadFailure: AbdException? = null
    private var moduleInstalled = false
    private var shouldDefer = false
    private var startResult = Result.success(false)
    private val methods = object : FilterMethodRepository by unexpected() {
        override suspend fun rootAvailable(): Boolean {
            rootAsked++
            return false
        }

        override suspend fun switchTo(method: FilterMethod): FilterMethodSwitch {
            done += "method $method"
            if (shouldDefer) userData.setRootSwitchPending()
            return if (shouldDefer) FilterMethodSwitch.RootPending else FilterMethodSwitch.Complete
        }

        override suspend fun retryRootSwitch(): Boolean {
            done += "retry root"
            if (!moduleInstalled) return false
            userData.setFilterMethod(FilterMethod.Root)
            return true
        }
    }

    private val daemon = object : DaemonRepository by unexpected() {
        override suspend fun status(): DaemonStatus {
            statusReads++
            statusFailure?.let { throw it }
            return DaemonStatus(
                running = true, pid = null, startedAt = null, version = null, apps = 0, rules = 1, hostRules = null,
                exclusions = null, paused = false, resumeAt = null, ipv6 = null, listsUpdated = null, urlFiltering = urlFiltering,
                cosmetic = cosmetic,
            )
        }

        override suspend fun setUrlFiltering(on: Boolean) {
            done += "urlFiltering $on"
            httpsWrite?.await()
            urlFiltering = on
        }

        override suspend fun setCosmetic(on: Boolean) {
            httpsWrite?.await()
            cosmetic = on
        }
    }

    private val viewModel by lazy {
        SettingsViewModel(
            platform = platform,
            userData = userData,
            daemon = daemon,
            module = object : ModuleRepository by unexpected() {
                override suspend fun state(): ModuleState {
                    moduleReadFailure?.let { throw it }
                    return if (moduleInstalled) ModuleState.Ready else ModuleState.NotInstalled
                }

                override suspend fun installBundled(): Boolean {
                    done += "install"
                    moduleInstalled = true
                    return true
                }

                override suspend fun startDaemon() = startResult.getOrThrow()
            },
            apps = object : AppsRepository by unexpected() {
                override suspend fun pinned(packageName: String?): List<PinnedHost> {
                    pinsFailure?.let { throw it }
                    return pinned
                }

                override suspend fun forgetPinned(packageName: String, host: String?) {
                    forgotten += packageName to host
                }
            },
            diagnostics = object : DiagnosticsRepository by unexpected() {
                override suspend fun exportLogs(destination: Uri) = logExport
            },
            methods = methods,
            https = object : HttpsRepository by unexpected() {
                override fun certificateInstalled() = true

                override suspend fun reload() {
                    done += "reload"
                }
            },
            vpn = vpn,
            io = main.testDispatcher,
            changes = changes,
        )
    }

    private fun TestScope.showSettings() = backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { viewModel.uiState.collect {} }
        .also { runCurrent() }

    private fun takeMessageText(): Int {
        val pending = viewModel.pendingMessages.value.first()
        viewModel.onMessageShown(pending.id)
        return pending.message.text
    }

    @Test
    fun givenUrlFilteringOn_whenSwitchedOff_thenItRespondsBeforeTheWriteAndSurvivesAStaleRead() = runTest {
        val write = CompletableDeferred<Unit>()
        httpsWrite = write
        showSettings()
        viewModel.setUrlFiltering(false)
        changes.notifyChanged()
        runCurrent()
        assertEquals(false, viewModel.uiState.value.service.status.valueOrNull?.urlFiltering)
        assertFalse(viewModel.uiState.value.service.busy)
        assertTrue(urlFiltering)

        write.complete(Unit)
        runCurrent()
        assertFalse(urlFiltering)
        assertEquals(false, viewModel.uiState.value.service.status.valueOrNull?.urlFiltering)
        assertEquals(R.string.feature_settings_url_filtering_off, takeMessageText())
    }

    @Test
    fun givenElementHidingOn_whenSwitchingOffFails_thenItRestoresTheSettingAndShowsTheFailure() = runTest {
        val write = CompletableDeferred<Unit>()
        httpsWrite = write
        showSettings()
        viewModel.setCosmetic(false)
        runCurrent()
        assertEquals(false, viewModel.uiState.value.service.status.valueOrNull?.cosmetic)
        assertTrue(cosmetic)

        write.completeExceptionally(AbdException.TimedOut())
        runCurrent()
        assertEquals(true, viewModel.uiState.value.service.status.valueOrNull?.cosmetic)
        assertEquals(R.string.feature_settings_service_timeout, takeMessageText())
    }

    @Test
    fun givenAnHttpsWriteIsPending_whenBothSwitchesArePressedAgain_thenTheLatestChoicesAreSaved() = runTest {
        val write = CompletableDeferred<Unit>()
        httpsWrite = write
        showSettings()
        viewModel.setUrlFiltering(false)
        runCurrent()
        viewModel.setCosmetic(false)
        viewModel.setUrlFiltering(true)
        runCurrent()
        assertEquals(true, viewModel.uiState.value.service.status.valueOrNull?.urlFiltering)
        assertEquals(false, viewModel.uiState.value.service.status.valueOrNull?.cosmetic)

        write.complete(Unit)
        runCurrent()
        assertTrue(urlFiltering)
        assertFalse(cosmetic)
        assertEquals(listOf("urlFiltering false", "urlFiltering true"), done)
        assertEquals(R.string.feature_settings_url_filtering_on, takeMessageText())
    }

    @Test
    fun givenRepeatedRestarts_whenTheFirstOutcomeIsAcknowledged_thenTheSecondRemains() = runTest {
        startResult = Result.success(true)
        viewModel.restartService()
        runCurrent()

        val first = viewModel.pendingMessages.first().single()
        assertEquals(R.string.feature_settings_restarted, first.message.text)
        assertEquals(first, viewModel.pendingMessages.first().single())

        viewModel.restartService()
        runCurrent()
        val second = viewModel.pendingMessages.value.last()
        assertEquals(R.string.feature_settings_restarted, second.message.text)
        assertTrue(first.id != second.id)
        viewModel.onMessageShown(first.id)
        viewModel.onMessageShown(first.id)
        assertEquals(listOf(second), viewModel.pendingMessages.value)
        viewModel.onMessageShown(second.id)
        assertTrue(viewModel.pendingMessages.value.isEmpty())
    }

    @Test
    fun withTheVpnNoRootCommandRunsAndRootIsAskedForOnce() = runTest {
        showSettings()
        val state = viewModel.uiState.value
        assertTrue(state.service.certificateInstalled)
        assertEquals(false, state.service.rootAvailable)
        viewModel.refresh()
        runCurrent()
        assertEquals(1, rootAsked)

        // Asked again when the user asks.
        viewModel.checkRoot()
        assertEquals(2, rootAsked)
    }

    @Test
    fun givenPinnedAppsAndDisabledNotifications_whenSettingsRefreshes_thenItUsesPlatformMetadata() = runTest {
        val personal = PinnedHost("com.example.news", "news.example", Instant.EPOCH)
        val missing = PinnedHost("com.example.missing", "missing.example", Instant.EPOCH)
        pinned = listOf(personal, personal.copy(host = "other.example"), personal.copy(profileSerial = 37), personal.copy(packageName = "@com.example.shared"), missing)
        notificationsAllowed = false

        showSettings()

        assertEquals("test version", viewModel.uiState.value.appVersion)
        assertFalse(viewModel.uiState.value.service.notificationsAllowed)
        assertEquals(listOf(setOf(personal.packageName, missing.packageName)), labelRequests)
        assertEquals(mapOf(personal.packageName to "News", missing.packageName to missing.packageName), viewModel.uiState.value.service.pinnedLabels)

        notificationsAllowed = true
        viewModel.refresh()
        runCurrent()

        assertTrue(viewModel.uiState.value.service.notificationsAllowed)
    }

    @Test
    fun switchingToRootDelegatesTheHandoffToTheMethodRepository() = runTest {
        showSettings()
        viewModel.switchToRoot()
        assertEquals(listOf("method Root"), done)
    }

    @Test
    fun aDeferredSwitchKeepsTheVpnAndRetriesWhenSettingsInstallsTheModule() = runTest {
        shouldDefer = true
        showSettings()
        viewModel.switchToRoot()
        assertEquals(FilterMethod.Vpn, userData.userData.value.method)
        assertTrue(userData.userData.value.rootSwitchPending)
        assertEquals(R.string.feature_settings_switch_root_pending, takeMessageText())

        viewModel.installModule()
        assertEquals(listOf("method Root", "install", "retry root"), done)
        assertEquals(FilterMethod.Root, userData.userData.value.method)
        assertEquals(R.string.feature_settings_switched_to_root, takeMessageText())
    }

    @Test
    fun givenAPendingSwitch_whenTheModuleProbeFailsAndIsRetried_thenReadinessIsReportedOnlyAfterVerification() = runTest {
        userData.setRootSwitchPending()
        moduleReadFailure = AbdException.TimedOut()
        showSettings()
        assertEquals(ModuleState.Unavailable, viewModel.uiState.value.service.module)

        moduleReadFailure = null
        moduleInstalled = true
        viewModel.checkRoot()
        runCurrent()

        assertEquals(ModuleState.Ready, viewModel.uiState.value.service.module)
        assertEquals(FilterMethod.Vpn, userData.userData.value.method)
        assertTrue(userData.userData.value.rootSwitchPending)
    }

    @Test
    fun aRestartDistinguishesADisabledModuleAndATimeoutFromSuccess() = runTest {
        userData.setFilterMethod(FilterMethod.Root)
        showSettings()
        viewModel.restartService()
        assertEquals(R.string.feature_settings_module_disabled, takeMessageText())

        startResult = Result.failure(AbdException.TimedOut())
        viewModel.restartService()
        assertEquals(R.string.feature_settings_service_timeout, takeMessageText())

        startResult = Result.success(true)
        viewModel.restartService()
        assertEquals(R.string.feature_settings_restarted, takeMessageText())
    }

    @Test
    fun givenTheCertificateWillBeRemoved_whenUrlFilteringIsTurnedOff_thenTheEngineReadsTheCertificateAgain() = runTest {
        showSettings()
        viewModel.turnOffUrlFiltering()
        assertEquals(listOf("urlFiltering false", "reload"), done)
    }

    @Test
    fun givenPersonalAndWorkPinsForOnePackage_whenForgotten_thenEachInstanceIsTargeted() = runTest {
        userData.setFilterMethod(FilterMethod.Root)
        val personal = PinnedHost("com.example.news", "personal.example", Instant.EPOCH)
        val work = PinnedHost("com.example.news", "work.example", Instant.EPOCH, profileSerial = 37)
        pinned = listOf(personal, work, work.copy(host = "other.example"))
        showSettings()

        viewModel.forgetPinned(work.appKey, work.host)
        runCurrent()
        viewModel.forgetAllPinned()
        runCurrent()

        assertEquals(
            listOf("37:com.example.news" to work.host, "com.example.news" to null, "37:com.example.news" to null),
            forgotten,
        )
    }

    @Test
    fun givenPinReadFails_whenSettingsRefreshes_thenUnavailableAndEmptyAreDistinct() = runTest {
        pinsFailure = AbdException.TimedOut()
        showSettings()

        val failed = viewModel.uiState.value.service
        assertEquals(LoadFailure.TimedOut, assertIs<ReadState.Unavailable>(failed.pinned).failure)
        assertTrue(assertIs<ReadState.Available<DaemonStatus>>(failed.status).value.running)
        assertTrue(failed.certificateInstalled)

        pinsFailure = null
        viewModel.refresh()
        runCurrent()

        assertTrue(assertIs<ReadState.Available<List<PinnedHost>>>(viewModel.uiState.value.service.pinned).value.isEmpty())
    }

    @Test
    fun givenStatusReadFails_whenRootSettingsLoads_thenThePinsStillLoad() = runTest {
        userData.setFilterMethod(FilterMethod.Root)
        statusFailure = AbdException.TimedOut()
        pinned = listOf(PinnedHost("com.example.news", "news.example", Instant.EPOCH))
        showSettings()

        val service = viewModel.uiState.value.service
        assertEquals(LoadFailure.TimedOut, assertIs<ReadState.Unavailable>(service.status).failure)
        assertEquals(pinned, assertIs<ReadState.Available<List<PinnedHost>>>(service.pinned).value)
    }

    @Test
    fun givenOnlyPartOfTheLogsWereExported_whenSettingsReportsTheResult_thenItShowsPartialSuccess() = runTest {
        showSettings()
        logExport = LogExportResult.DaemonIncomplete

        viewModel.exportLogs(Uri.parse("content://example/logs"))

        assertEquals(R.string.feature_settings_logs_exported_partial, takeMessageText())
    }

    @Test
    fun daemonChangesReloadSettingsOnlyWhileShown() = runTest {
        val visible = showSettings()
        assertEquals(1, statusReads)
        changes.notifyChanged()
        runCurrent()
        assertEquals(2, statusReads)

        visible.cancel()
        advanceTimeBy(5_000)
        runCurrent()
        changes.notifyChanged()
        runCurrent()
        assertEquals(2, statusReads)

        showSettings()
        assertEquals(3, statusReads)
    }
}
