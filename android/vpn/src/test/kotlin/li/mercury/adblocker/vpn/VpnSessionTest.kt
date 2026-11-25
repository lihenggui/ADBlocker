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

package li.mercury.adblocker.vpn

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.engine.PersonalAppIdentity
import li.mercury.adblocker.core.engine.VpnBinding
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.LoadFailure
import li.mercury.adblocker.core.model.ReadState
import li.mercury.adblocker.core.model.VpnState
import li.mercury.adblocker.core.testing.TestVpnAppRepository
import java.time.Duration
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class VpnSessionTest {
    /** The system's side of the VPN: what was built, shown and stopped. */
    private class FakeHost : VpnHost {
        val installed = mutableMapOf<String, Int>()
        val built = mutableListOf<Set<String>>()
        override fun personalAppIdentities(): List<PersonalAppIdentity> {
            inventories++
            return installed.filter { (key, uid) -> key != OWN && uid in 10_000..19_999 }
                .map { (key, uid) -> PersonalAppIdentity(key, uid, setOf(key)) }
        }

        var inventories = 0
        var listAccess = false

        override fun appListAccess() = listAccess

        val closed = mutableListOf<Int>()
        val shown = mutableListOf<Shown?>()
        val finished = mutableListOf<Int>()
        val interruptions = mutableListOf<Boolean>()
        var revokedNotices = 0
        var givesDescriptor = true
        var isPrepared = true
        private var nextFd = 100

        override var alwaysOn = false
        override var lockdown = false
        override var networkHandle: Long? = 1L
        override var dnsServers: List<String> = emptyList()
        val builtWithVirtualDns = mutableListOf<Boolean>()
        override var ipv6 = true
        val builtIpv6 = mutableListOf<Boolean>()

        override fun uidOf(packageName: String): Int? = installed[packageName]

        override fun establish(apps: Set<String>, ipv6: Boolean, virtualDns: Boolean): Int? {
            built += apps
            builtIpv6 += ipv6
            builtWithVirtualDns += virtualDns
            return if (givesDescriptor) nextFd++ else null
        }

        override fun close(fd: Int) {
            closed += fd
        }

        override fun protect(fd: Int) = true

        override fun prepared() = isPrepared

        override fun show(shown: Shown?) {
            this.shown += shown
        }

        override fun filteringEnded(interrupted: Boolean) {
            interruptions += interrupted
        }

        override fun noticeRevoked() {
            revokedNotices++
        }

        override fun finish(startId: Int) {
            finished += startId
        }
    }

    /** The engine's stack: which descriptor it runs, if any. */
    private class FakeEngine : VpnEngine {
        override suspend fun available() = true
        override var binding: VpnBinding? = null
        val started = mutableListOf<Int>()
        val startedDns = mutableListOf<List<String>>()
        var running: Int? = null
        var stops = 0
        var answer = 0
        var throwsOnStart = false

        /** Holds [stop] until it completes, as an engine that takes time to let go. */
        var stopGate: CompletableDeferred<Unit>? = null

        /** Runs while the engine stops, as a stack that ends meanwhile would tell. */
        var whileStopping: (() -> Unit)? = null

        override suspend fun start(tunFd: Int, ipv6: Boolean, dnsServers: List<String>): Int {
            if (throwsOnStart) throw IllegalStateException("the engine could not open")
            started += tunFd
            startedDns += dnsServers
            if (answer == 0) running = tunFd
            return answer
        }

        override suspend fun <T> withPersonalAppSnapshot(apps: List<PersonalAppIdentity>, refresh: suspend () -> T): T = refresh()

        override suspend fun stop() {
            stopGate?.await()
            if (running != null) whileStopping?.invoke()
            stops++
            running = null
        }
    }

    private class Vpn(test: TestScope) {
        val host = FakeHost()
        val engine = FakeEngine()
        val running = RunningVpn()
        val method = MutableStateFlow(FilterMethod.Vpn)
        val changes = MutableSharedFlow<Unit>(extraBufferCapacity = 1)
        val apps = TestVpnAppRepository(status(paused = false, resumeAt = null))
        var reloads = 0
        var wallClockOffsetMs = 0L

        val session = VpnSession(
            host = host,
            engine = engine,
            vpnApps = apps,
            reload = { reloads++ },
            method = method,
            changes = changes,
            running = running,
            ownPackage = OWN,
            scope = test.backgroundScope,
            now = { NOW.plusMillis(test.testScheduler.currentTime + wallClockOffsetMs) },
        )

        val state get() = running.status.value.state

        fun choose(vararg packages: String) {
            apps.chosen = packages.map { FilteredApp(it, FilterLevel.Full, pausedUntil = null) }
        }
    }

    /** Tells the session of a change, as DataChanges does, and lets it act on it. */
    private suspend fun TestScope.changed(vpn: Vpn) {
        runCurrent()
        vpn.changes.emit(Unit)
        runCurrent()
    }

    @Test
    fun givenTheVpnIsRunning_whenTheStatusReadFails_thenTheNotificationDoesNotClaimFilteringIsActive() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        val active = vpn.host.shown.last()

        vpn.apps.statusFailure = AbdException.TimedOut()
        changed(vpn)

        val unavailable = vpn.host.shown.last()
        assertNotEquals(active, unavailable)
        assertEquals(ReadState.Unavailable(LoadFailure.TimedOut), unavailable?.status)
        assertEquals(VpnState.On, vpn.state)
        assertEquals(listOf(100), vpn.engine.started)

        vpn.apps.statusFailure = null
        changed(vpn)
        assertEquals(active, vpn.host.shown.last())
    }

    @Test
    fun givenAnEstablishedVpn_whenTheServiceIsDestroyed_thenTheInterruptionIsRecorded() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.session.destroyed()

        assertEquals(VpnState.Off, vpn.state)
        assertEquals(listOf(true), vpn.host.interruptions)
    }

    @Test
    fun givenAnEstablishedVpn_whenStoppedOrRevoked_thenThereIsNoInterruption() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.session.stop()
        vpn.session.destroyed()
        vpn.session.start(2)
        vpn.session.revoked()
        vpn.session.destroyed()

        assertEquals(VpnState.Revoked, vpn.state)
        assertEquals(listOf(false, false), vpn.host.interruptions)
    }

    @Test
    fun givenAllPersonalApps_whenAnExclusionOrInstallationChanges_thenOnlyEligibleAppsAreRouted() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001, OWN to 10_000, "android" to 1_000)
        vpn.apps.selections = AppSelections(FilteringScope.All)
        vpn.apps.appsFailure = AbdException.CommandFailed("app list") // All apps uses the personal inventory directly.
        vpn.session.start(1)
        assertEquals(1, vpn.host.inventories)
        assertEquals(listOf(setOf(NEWS)), vpn.host.built)

        vpn.host.installed[SHOP] = 10_002
        vpn.session.packagesChanged()
        runCurrent()
        assertEquals(setOf(NEWS, SHOP), vpn.host.built.last())

        vpn.apps.selections = vpn.apps.selections.copy(excluded = setOf(SHOP))
        changed(vpn)
        assertEquals(listOf(setOf(NEWS), setOf(NEWS, SHOP), setOf(NEWS)), vpn.host.built)
    }

    @Test
    fun givenAllPersonalApps_whenOnlyTheNetworkOrASettingChanges_thenThePackagesAreNotInventoriedAgain() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001)
        vpn.apps.selections = AppSelections(FilteringScope.All)
        vpn.session.start(1)
        assertEquals(1, vpn.host.inventories)

        changed(vpn)
        vpn.host.ipv6 = false
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(1, vpn.host.inventories)

        vpn.host.installed[SHOP] = 10_002
        vpn.session.packagesChanged()
        runCurrent()
        assertEquals(2, vpn.host.inventories)
        assertEquals(setOf(NEWS, SHOP), vpn.host.built.last())

        changed(vpn)
        assertEquals(2, vpn.host.inventories)
    }

    @Test
    fun givenAllPersonalAppsWithTheAppListWithheld_whenTheListIsAllowed_thenTheNextRefreshRoutesTheAppsItShows() = runTest {
        val vpn = Vpn(this)
        vpn.apps.selections = AppSelections(FilteringScope.All)
        vpn.session.start(1)
        // Nothing to route while the system shows no personal app: no VPN, never one for every app.
        assertEquals(emptyList(), vpn.host.built)
        changed(vpn)
        assertEquals(1, vpn.host.inventories)

        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002)
        vpn.host.listAccess = true
        changed(vpn)

        assertEquals(2, vpn.host.inventories)
        assertEquals(1, vpn.reloads)
        assertEquals(listOf(setOf(NEWS, SHOP)), vpn.host.built)
    }

    @Test
    fun givenAllPersonalApps_whenTheVpnEndsAndStartsAgain_thenThePackagesAreInventoriedAgain() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001)
        vpn.apps.selections = AppSelections(FilteringScope.All)
        vpn.session.start(1)
        vpn.session.stop()

        vpn.host.installed[SHOP] = 10_002
        vpn.session.start(2)

        assertEquals(2, vpn.host.inventories)
        assertEquals(setOf(NEWS, SHOP), vpn.host.built.last())
    }

    @Test
    fun onlyTheChosenAppsThatAreInstalledAreRoutedAndNeverThisApp() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS, SHOP, OWN, "com.example.uninstalled")
        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002, OWN to 10_000)
        vpn.session.start(1)
        assertEquals(listOf(setOf(NEWS, SHOP)), vpn.host.built)
        assertEquals(listOf(100), vpn.engine.started)
        assertEquals(VpnState.On, vpn.state)
        assertEquals(Shown(apps = 2, status = ReadState.Available(vpn.apps.daemonStatus)), vpn.host.shown.last())
    }

    @Test
    fun givenOnlyAWorkApp_whenTheVpnStarts_thenThePersonalInstanceIsNeverRouted() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed[NEWS] = 10_001
        vpn.apps.chosen = listOf(FilteredApp(NEWS, FilterLevel.Full, pausedUntil = null, profileSerial = 37))
        vpn.session.start(1)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(emptyList(), vpn.engine.started)
    }

    @Test
    fun givenARootSharedGroup_whenTheVpnStarts_thenTheGroupIsNeverResolvedAsAPackage() = runTest {
        val vpn = Vpn(this)
        val group = "@com.example.suite"
        vpn.host.installed[group] = 10_001
        vpn.apps.chosen = listOf(FilteredApp(group, FilterLevel.Full, pausedUntil = null))
        vpn.session.start(1)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(emptyList(), vpn.engine.started)
    }

    @Test
    fun noVpnIsBuiltWhileNoAppIsChosen() = runTest {
        // A VPN that allows no app would take the traffic of every app.
        val vpn = Vpn(this)
        vpn.choose(OWN)
        vpn.host.installed[OWN] = 10_000
        vpn.session.start(1)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(emptyList(), vpn.engine.started)
        assertEquals(VpnState.On, vpn.state)
        assertEquals(0, vpn.host.shown.last()?.apps)
    }

    @Test
    fun anAppChosenLaterBuildsTheVpnThen() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.choose(NEWS)
        changed(vpn)
        assertEquals(listOf(setOf(NEWS)), vpn.host.built)
        assertEquals(100, vpn.engine.running)
    }

    @Test
    fun aChangeToTheChosenAppsBuildsTheVpnAgain() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002)
        vpn.choose(NEWS)
        vpn.session.start(1)
        vpn.choose(NEWS, SHOP)
        changed(vpn)
        assertEquals(listOf(setOf(NEWS), setOf(NEWS, SHOP)), vpn.host.built)
        assertEquals(listOf(100, 101), vpn.engine.started)
        assertEquals(101, vpn.engine.running)
        assertEquals(VpnState.On, vpn.state)
    }

    @Test
    fun givenABuildIsCancelledWhileTheOldStackStops_whenTheStackHasStopped_thenTheNewDescriptorIsStillHandedToTheEngine() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002)
        vpn.choose(NEWS)
        vpn.session.start(1)
        vpn.choose(NEWS, SHOP)
        val gate = CompletableDeferred<Unit>()
        vpn.engine.stopGate = gate

        val caller = launch { vpn.session.refresh() }
        runCurrent()
        caller.cancel()
        gate.complete(Unit)
        runCurrent()

        assertEquals(listOf(100, 101), vpn.engine.started)
        assertEquals(101, vpn.engine.running)
        assertEquals(emptyList(), vpn.host.closed)
        assertEquals(VpnState.On, vpn.state)
    }

    @Test
    fun pausingAnAppOrChangingItsModeBuildsNothing() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002)
        vpn.choose(NEWS, SHOP)
        vpn.session.start(1)
        vpn.apps.chosen = listOf(
            FilteredApp(NEWS, FilterLevel.Full, pausedUntil = NOW.plus(Duration.ofHours(1))),
            FilteredApp(SHOP, FilterLevel.Hosts, pausedUntil = null),
        )
        changed(vpn)
        vpn.apps.daemonStatus = status(paused = true, resumeAt = null)
        changed(vpn)
        assertEquals(1, vpn.host.built.size)
        assertEquals(listOf(100), vpn.engine.started)
    }

    @Test
    fun aPackageInstalledOrRemovedHasTheEngineReadThePackagesFirst() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS, SHOP)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.host.installed[SHOP] = 10_002
        vpn.session.packagesChanged()
        runCurrent()
        assertEquals(1, vpn.reloads)
        assertEquals(listOf(setOf(NEWS), setOf(NEWS, SHOP)), vpn.host.built)
    }

    @Test
    fun anAppInstalledAgainUnderAnotherUidBuildsTheVpnAgain() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.host.installed[NEWS] = 10_077
        vpn.session.packagesChanged()
        runCurrent()
        assertEquals(listOf(setOf(NEWS), setOf(NEWS)), vpn.host.built)
    }

    @Test
    fun theVpnCarriesIpv6OnlyWhileTheNetworkBeneathHasIt() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.host.ipv6 = false
        vpn.session.start(1)
        vpn.host.ipv6 = true
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        // Unchanged: nothing is built again.
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(listOf(false, true), vpn.host.builtIpv6)
        assertEquals(VpnState.On, vpn.state)
    }

    @Test
    fun aNetworkChangeBuildsNoVpnForNoApp() = runTest {
        val vpn = Vpn(this)
        vpn.session.start(1)
        vpn.host.ipv6 = false
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(emptyList(), vpn.host.built)
    }

    @Test
    fun aNewUnderlyingNetworkReplacesUdpFlowsEvenWithTheSameAddressFamilies() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.host.networkHandle = 2L
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(listOf(true, true), vpn.host.builtIpv6)
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(2, vpn.host.built.size)
        vpn.session.stop()
    }

    @Test
    fun givenTheDefaultNetworkIsLost_whenAReplacementArrivesWithinTheDebounce_thenOnlyTheReplacementIsRouted() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)

        vpn.host.networkHandle = null
        vpn.host.ipv6 = false
        vpn.session.networkChanged()
        advanceTimeBy(200)
        assertEquals(listOf(true), vpn.host.builtIpv6)

        vpn.host.networkHandle = 2L
        vpn.host.ipv6 = true
        vpn.host.dnsServers = listOf("192.0.2.53:53")
        vpn.session.networkChanged()
        advanceTimeBy(200)
        runCurrent()
        assertEquals(listOf(emptyList()), vpn.engine.startedDns)

        advanceTimeBy(400)
        runCurrent()
        assertEquals(listOf(true, true), vpn.host.builtIpv6)
        assertEquals(listOf(emptyList(), listOf("192.0.2.53:53")), vpn.engine.startedDns)
    }

    @Test
    fun givenNewResolvers_whenLinkPropertiesChange_thenTheEngineReceivesTheNewList() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.host.dnsServers = listOf("192.0.2.53:53", "[2001:db8::53]:53")
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        vpn.host.dnsServers = listOf("198.51.100.53:53")
        vpn.session.networkChanged()
        advanceTimeBy(500)
        runCurrent()
        assertEquals(listOf(false, true, true), vpn.host.builtWithVirtualDns)
        assertEquals(
            listOf(emptyList(), listOf("192.0.2.53:53", "[2001:db8::53]:53"), listOf("198.51.100.53:53")),
            vpn.engine.startedDns,
        )
        vpn.session.stop()
    }

    @Test
    fun theLastAppTakenOutTakesTheVpnDown() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.choose()
        changed(vpn)
        assertNull(vpn.engine.running)
        assertEquals(1, vpn.host.built.size)
        // It waits for an app to be chosen again.
        assertEquals(VpnState.On, vpn.state)
    }

    @Test
    fun revokedStopsTheEngineAndTellsTheUserOnce() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(3)
        vpn.session.revoked()
        vpn.session.revoked()
        assertEquals(VpnState.Revoked, vpn.state)
        assertNull(vpn.engine.running)
        assertNull(vpn.engine.binding)
        assertEquals(1, vpn.host.revokedNotices)
        assertEquals(listOf(3), vpn.host.finished)
    }

    @Test
    fun theEngineStoppingByItselfFailsTheVpnAndStopsTheService() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.engine.binding!!.stopped(1)
        runCurrent()
        assertEquals(VpnState.Failed, vpn.state)
        assertNull(vpn.engine.running)
        assertEquals(listOf(1), vpn.host.finished)
        assertEquals(0, vpn.host.revokedNotices)
    }

    @Test
    fun theEngineStoppingOnceAnotherVpnAppTookOverIsARevoke() = runTest {
        // The stack ends as soon as the system takes the VPN, before onRevoke comes.
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.host.isPrepared = false
        vpn.engine.binding!!.stopped(0)
        runCurrent()
        vpn.session.revoked()
        assertEquals(VpnState.Revoked, vpn.state)
        assertEquals(1, vpn.host.revokedNotices)
    }

    @Test
    fun theOldStackEndingWhileTheVpnIsBuiltAgainIsNoFailure() = runTest {
        val vpn = Vpn(this)
        vpn.host.installed += mapOf(NEWS to 10_001, SHOP to 10_002)
        vpn.choose(NEWS)
        vpn.session.start(1)
        // The old descriptor stops working once the new VPN is built.
        vpn.engine.whileStopping = { vpn.engine.binding?.stopped(1) }
        vpn.choose(NEWS, SHOP)
        changed(vpn)
        assertEquals(VpnState.On, vpn.state)
        assertEquals(101, vpn.engine.running)
        assertEquals(emptyList(), vpn.host.finished)
    }

    @Test
    fun anEngineThatDoesNotStartEndsTheVpn() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.engine.answer = 1
        vpn.session.start(1)
        assertEquals(VpnState.Failed, vpn.state)
        assertEquals(listOf(1), vpn.host.finished)
        assertNull(vpn.engine.binding)
        // The engine took the descriptor, and closes it.
        assertEquals(emptyList(), vpn.host.closed)
    }

    @Test
    fun aDescriptorTheEngineNeverTookIsClosed() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.engine.throwsOnStart = true
        vpn.session.start(1)
        assertEquals(listOf(100), vpn.host.closed)
        assertEquals(VpnState.Failed, vpn.state)
    }

    @Test
    fun noDescriptorOnceConsentIsGoneIsARevoke() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.host.givesDescriptor = false
        vpn.host.isPrepared = false
        vpn.session.start(1)
        assertEquals(VpnState.Revoked, vpn.state)
        assertEquals(emptyList(), vpn.engine.started)
    }

    @Test
    fun chosenAppsThatCannotBeReadAtTheStartEndTheVpn() = runTest {
        val vpn = Vpn(this)
        vpn.apps.appsFailure = AbdException.CommandFailed("app list")
        vpn.session.start(1)
        assertEquals(VpnState.Failed, vpn.state)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(listOf(1), vpn.host.finished)
    }

    @Test
    fun givenUnreadableSelections_whenTheVpnStarts_thenItFailsWithoutRoutingApps() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.apps.selectionsFailure = AbdException.CommandFailed("app selections")

        vpn.session.start(1)

        assertEquals(VpnState.Failed, vpn.state)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(emptyList(), vpn.engine.started)
        assertEquals(listOf(1), vpn.host.finished)
    }

    @Test
    fun givenAnActiveVpn_whenItsSelectionBecomesUnreadable_thenTheStackAndServiceStop() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        assertEquals(100, vpn.engine.running)

        vpn.apps.selectionsFailure = AbdException.CommandFailed("app selections")
        changed(vpn)

        assertEquals(VpnState.Failed, vpn.state)
        assertEquals(null, vpn.engine.running)
        assertEquals(null, vpn.engine.binding)
        assertEquals(listOf(1), vpn.host.finished)
    }

    @Test
    fun givenAnUnrecognizedStart_whenTheVpnIsIdle_thenItStopsTheService() = runTest {
        val vpn = Vpn(this)
        var stopped = false

        vpn.session.unrecognizedStart(1) { stopped = true }

        assertTrue(stopped)
        assertEquals(VpnState.Off, vpn.state)
    }

    @Test
    fun givenAnUnrecognizedStart_whenTheVpnIsActive_thenItKeepsRunningAndFinishesWithTheLatestId() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)

        vpn.session.unrecognizedStart(2) { error("active service stopped") }

        assertEquals(VpnState.On, vpn.state)
        vpn.session.stop()
        assertEquals(listOf(2), vpn.host.finished)
    }

    @Test
    fun aRunningVpnThatCannotReadTheChosenAppsGoesOnAsItIs() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        vpn.apps.appsFailure = AbdException.CommandFailed("app list")
        changed(vpn)
        assertEquals(VpnState.On, vpn.state)
        assertEquals(100, vpn.engine.running)
    }

    @Test
    fun theVpnDoesNotStartForTheRootModule() = runTest {
        val vpn = Vpn(this)
        vpn.method.value = FilterMethod.Root
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        assertEquals(VpnState.Off, vpn.state)
        assertEquals(emptyList(), vpn.host.built)
        assertEquals(listOf(1), vpn.host.finished)
    }

    @Test
    fun choosingTheRootModuleStopsTheVpn() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        runCurrent()
        vpn.method.value = FilterMethod.Root
        runCurrent()
        assertEquals(VpnState.Off, vpn.state)
        assertNull(vpn.engine.running)
        assertEquals(listOf(1), vpn.host.finished)
    }

    @Test
    fun stoppingEndsTheVpnAndTheServiceOfTheLatestStart() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        // Started again while on, as Always-on VPN or a second tap would.
        vpn.session.start(2)
        vpn.session.stop()
        assertEquals(1, vpn.host.built.size)
        assertEquals(VpnState.Off, vpn.state)
        assertNull(vpn.engine.running)
        assertEquals(listOf(2), vpn.host.finished)
    }

    @Test
    fun alwaysOnAndLockdownAreReported() = runTest {
        val vpn = Vpn(this)
        vpn.host.alwaysOn = true
        vpn.host.lockdown = true
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.session.start(1)
        assertTrue(vpn.running.status.value.alwaysOn)
        assertTrue(vpn.running.status.value.lockdown)
        // Kept once it stopped: they say what the apps have without it.
        vpn.engine.binding!!.stopped(1)
        runCurrent()
        assertTrue(vpn.running.status.value.lockdown)
    }

    @Test
    fun aPauseIsShownUntilItEnds() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        val resumeAt = NOW.plus(Duration.ofMinutes(60))
        vpn.apps.daemonStatus = status(paused = true, resumeAt = resumeAt)
        vpn.session.start(1)
        assertEquals(Shown(apps = 1, status = ReadState.Available(vpn.apps.daemonStatus)), vpn.host.shown.last())
        // The engine ends it by itself, and tells nobody.
        vpn.apps.daemonStatus = status(paused = false, resumeAt = null)
        advanceTimeBy(Duration.ofMinutes(61).toMillis())
        runCurrent()
        assertEquals(Shown(apps = 1, status = ReadState.Available(vpn.apps.daemonStatus)), vpn.host.shown.last())
        assertEquals(1, vpn.host.built.size)
    }

    @Test
    fun givenASleepPastThePauseDeadline_whenTheScreenTurnsOn_thenTheNotificationReadsTheCurrentStatus() = runTest {
        val vpn = Vpn(this)
        vpn.choose(NEWS)
        vpn.host.installed[NEWS] = 10_001
        vpn.apps.daemonStatus = status(paused = true, resumeAt = NOW.plusSeconds(60))
        vpn.session.start(1)
        runCurrent()

        vpn.wallClockOffsetMs = 61_000
        vpn.apps.daemonStatus = status(paused = false, resumeAt = null)
        vpn.session.clockChanged()
        runCurrent()

        assertEquals(Shown(apps = 1, status = ReadState.Available(vpn.apps.daemonStatus)), vpn.host.shown.last())
        assertEquals(1, vpn.host.built.size)
    }

    @Test
    fun givenTheClockMovesBack_whenTheOldPauseTimerWakes_thenItWaitsForTheNewDeadline() = runTest {
        val vpn = Vpn(this)
        vpn.apps.daemonStatus = status(paused = true, resumeAt = NOW.plusSeconds(60))
        vpn.session.start(1)
        runCurrent()
        val shown = vpn.host.shown.toList()

        vpn.wallClockOffsetMs = -30_000
        advanceTimeBy(61_000)
        runCurrent()
        assertEquals(shown, vpn.host.shown)

        vpn.apps.daemonStatus = status(paused = false, resumeAt = null)
        advanceTimeBy(30_000)
        runCurrent()
        assertEquals(Shown(apps = 0, status = ReadState.Available(vpn.apps.daemonStatus)), vpn.host.shown.last())
    }

    private companion object {
        const val OWN = "li.mercury.adblocker"
        const val NEWS = "com.example.news"
        const val SHOP = "com.example.shop"
        val NOW: Instant = Instant.parse("2026-11-19T12:00:00Z")

        fun status(paused: Boolean, resumeAt: Instant?) = DaemonStatus(
            running = true,
            pid = null,
            startedAt = null,
            version = null,
            apps = null,
            rules = null,
            hostRules = null,
            exclusions = null,
            paused = paused,
            resumeAt = resumeAt,
            ipv6 = null,
            listsUpdated = null,
        )
    }
}
