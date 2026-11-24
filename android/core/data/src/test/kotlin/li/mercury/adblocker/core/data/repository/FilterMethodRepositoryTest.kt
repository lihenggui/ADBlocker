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

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.FilterCatalog
import li.mercury.adblocker.core.data.repository.FilterMethodRepositoryFixture.FakeAbd
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.testing.unexpected
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class FilterMethodRepositoryTest {
    private val fixture = FilterMethodRepositoryFixture()

    @get:Rule
    val temporaryFolder = TemporaryFolder()

    @Test
    fun theChosenAppsListsRulesAndTrustedSitesAreCarriedIntoTheVpn() = runTest {
        val root = FakeAbd(
            apps = setOf("com.example.news", "com.example.bank"),
            lists = mapOf(EASYLIST to true, CHINA to false),
            trusted = listOf("bank.example"),
            rules = "||ads.example^\n",
        )
        val engine = FakeAbd(urlFiltering = false)
        fixture.repository(root, engine).switchTo(FilterMethod.Vpn)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(setOf("com.example.news", "com.example.bank"), engine.apps)
        assertEquals(mapOf(EASYLIST to true, CHINA to false), engine.lists)
        assertEquals(listOf("bank.example"), engine.trusted)
        assertEquals("||ads.example^\n", engine.rules)
        assertEquals(1, fixture.updatesAsked)
    }

    @Test
    fun givenWorkAndPersonalSelections_whenSwitchingMethods_thenOnlyPersonalAppsAreCarried() = runTest {
        val root = FakeAbd(apps = setOf("com.example.news", "37:com.example.news", "37:com.example.work"))
        val engine = FakeAbd(apps = setOf("com.example.work"))
        val repo = fixture.repository(root, engine)

        repo.switchTo(FilterMethod.Vpn)
        assertEquals(setOf("com.example.news"), engine.apps)
        assertTrue(engine.ran.none { "37:" in it })

        repo.switchTo(FilterMethod.Root)
        assertEquals(setOf("com.example.news", "37:com.example.news", "37:com.example.work"), root.apps)
        assertTrue(root.ran.none { "37:" in it })
    }

    @Test
    fun givenSavedGroupsAndLegacyProfileKeys_whenSwitchingToVpn_thenOnlyPersonalPackagesAreCarried() = runTest {
        val personal = "com.example.news"
        val root = FakeAbd(
            apps = setOf(personal, "@com.example.suite", "37:com.example.work", "37:@com.example.suite", "037:com.example.legacy", "@"),
        )
        val savedWork = setOf("42:com.example.saved", "42:@com.example.savedgroup", "042:com.example.legacy")
        val engine = FakeAbd(apps = savedWork + "com.example.old")

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(savedWork + personal, engine.apps)
        val appWrites = engine.ran.filter { it.startsWith("app add ") || it.startsWith("app remove ") }
        assertEquals(listOf("app remove com.example.old", "app add $personal"), appWrites)
    }

    @Test
    fun givenRejectedPersonalKey_whenSwitchingToVpn_thenThePreviousSelectionIsRestored() = runTest {
        val root = FakeAbd(apps = setOf(""))
        val prior = setOf("com.example.old", "42:com.example.saved")
        val engine = FakeAbd(apps = prior).apply {
            beforeCommand = { if (it == "app add ") throw AbdException.CommandRejected() }
        }

        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Apps)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(prior, engine.apps)
    }

    @Test
    fun whatTheNewMethodHadThatTheUserNoLongerChoseIsTakenOut() = runTest {
        // Chosen under the VPN once, then taken out under the root module.
        val root = FakeAbd(apps = setOf("com.example.news"), lists = mapOf(EASYLIST to true))
        val engine = FakeAbd(
            apps = setOf("com.example.news", "com.example.old"),
            lists = mapOf(EASYLIST to false, CHINA to true),
            trusted = listOf("old.example"),
        )
        fixture.repository(root, engine).switchTo(FilterMethod.Vpn)
        assertEquals(setOf("com.example.news"), engine.apps)
        assertEquals(mapOf(EASYLIST to true), engine.lists)
        assertEquals(emptyList(), engine.trusted)
        // Nothing added by address alone: nothing to download.
        assertEquals(0, fixture.updatesAsked)
    }

    @Test
    fun anItemThatCannotBeWrittenDoesNotKeepTheOthersFromBeingCarried() = runTest {
        val root = FakeAbd(
            apps = setOf("com.example.news", "com.example.shop"),
            trusted = listOf("bank.example"),
        )
        val engine = FakeAbd().apply { failing += "allow add bank.example" }
        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.TrustedSites)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))
        assertEquals(root.apps, engine.apps)
        assertTrue(engine.trusted.isEmpty())
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
    }

    @Test
    fun givenRootOrItsModuleIsUnavailable_whenChoosingVpn_thenTheSwitchIsComplete() = runTest {
        for (absent in listOf(AbdException.NoRoot(), AbdException.ModuleMissing())) {
            for (settings in listOf(UserData(), UserData(filterMethod = FilterMethod.Root, onboardingDone = true))) {
                fixture.userData.userData.value = settings
                val root = FakeAbd().apply { unreachable = absent }
                val engine = FakeAbd(apps = setOf("com.example.shop"))

                assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

                assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.filterMethod)
                assertEquals(setOf("com.example.shop"), engine.apps)
                assertEquals(emptyList(), engine.ran)
            }
        }
    }

    @Test
    fun givenNoRootSetupAndAnUnreadableModule_whenChoosingVpn_thenTheSwitchIsComplete() = runTest {
        fixture.userData.userData.value = UserData()
        val root = FakeAbd().apply { unreachable = AbdException.TimedOut() }
        val engine = FakeAbd()

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.filterMethod)
        assertTrue(engine.ran.isEmpty())
    }

    @Test
    fun givenCompletedLegacyRootSetupAndAnUnreadableModule_whenChoosingVpn_thenTheTransferIsIncomplete() = runTest {
        fixture.userData.userData.value = UserData(onboardingDone = true)
        val root = FakeAbd(apps = setOf("com.example.news")).apply { unreachable = AbdException.TimedOut() }
        val engine = FakeAbd()

        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.filterMethod)
        assertTrue(engine.ran.isEmpty())
    }

    @Test
    fun givenAnUnconfiguredModuleWithBundledDefaults_whenChoosingVpn_thenNoVpnSettingsAreReplaced() = runTest {
        fixture.userData.userData.value = UserData()
        val defaults = FilterCatalog.setupLists(listOf("zh")).filterValues { it } +
            ("https://filters.adtidy.org/extension/ublock/filters/224.txt" to true)
        val root = FakeAbd(lists = defaults).apply { failing += "rules get" }
        val engine = FakeAbd(apps = setOf("com.example.saved")).apply { unreachable = AbdException.EngineMissing() }

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.filterMethod)
        assertEquals(setOf("com.example.saved"), engine.apps)
    }

    @Test
    fun givenAStoredVpnChoice_whenSetupResumesAfterAnUpgrade_thenNeitherConfigurationIsRead() = runTest {
        val directory = temporaryFolder.newFolder()
        fixture.withStoredPreferences(directory) { it.setFilterMethod(FilterMethod.Vpn) }
        val root = FakeAbd().apply { beforeCommand = { error("VPN was already chosen") } }
        val engine = FakeAbd().apply { beforeCommand = { error("Nothing to carry") } }

        fixture.withStoredPreferences(directory) { stored ->
            assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine, stored).switchTo(FilterMethod.Vpn))
            assertEquals(FilterMethod.Vpn, stored.userData.first().filterMethod)
        }
    }

    @Test
    fun givenNoAppChoicesButCustomRulesAndLists_whenBothFail_thenBothPartsAreReported() = runTest {
        fixture.userData.userData.value = UserData()
        val root = FakeAbd(lists = mapOf(EASYLIST to false), rules = "||ads.example^\n")
        val engine = FakeAbd().apply { failing += setOf("list add", "rules set") }

        assertEquals(
            FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Lists, VpnTransferPart.Rules)),
            fixture.repository(root, engine).switchTo(FilterMethod.Vpn),
        )
    }

    @Test
    fun givenIdenticalUrlSettings_whenSwitchingToVpn_thenNoUrlSettingIsWritten() = runTest {
        val root = FakeAbd(apps = setOf("com.example.news"))
        val engine = FakeAbd().apply { failing += setOf("url-filtering", "cosmetic") }

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(root.apps, engine.apps)
    }

    @Test
    fun givenNoMethodChosenAndARunningModule_whenChoosingVpn_thenItsSettingsAreCarried() = runTest {
        fixture.userData.userData.value = UserData()
        val root = FakeAbd(apps = setOf("com.example.news"), lists = mapOf(EASYLIST to true))
        val engine = FakeAbd()

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(setOf("com.example.news"), engine.apps)
        assertEquals(mapOf(EASYLIST to true), engine.lists)
    }

    @Test
    fun givenVpnSelectionCannotBeRead_whenSwitchingFromRoot_thenNoVpnAppPolicyIsReplaced() = runTest {
        val root = FakeAbd(
            apps = setOf("com.example.new"),
            filteringScope = FilteringScope.All,
            excluded = setOf("com.example.bank"),
        )
        val previous = setOf("com.example.old")
        val engine = FakeAbd(apps = previous, excluded = setOf("com.example.notes")).apply {
            failing += "app selections --json"
        }

        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Apps)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(previous, engine.apps)
        assertEquals(setOf("com.example.notes"), engine.excluded)
        assertEquals(FilteringScope.Chosen, engine.filteringScope)
    }

    @Test
    fun givenRootExclusionsChangeDuringCopy_whenSwitchingToVpn_thenItsSavedPolicyIsKept() = runTest {
        val root = FakeAbd(filteringScope = FilteringScope.All, excluded = setOf("com.example.bank"))
        val previousExclusions = setOf("com.example.notes")
        val engine = FakeAbd(excluded = previousExclusions)
        engine.beforeCommand = { if (it == "rules get") root.excluded += "com.example.shop" }

        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Apps)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(previousExclusions, engine.excluded)
        assertEquals(FilteringScope.Chosen, engine.filteringScope)
    }

    @Test
    fun givenVpnScopeWriteFails_whenSwitchingFromRoot_thenThePreviousPolicyIsRestoredBeforePublication() = runTest {
        val root = FakeAbd(
            apps = setOf("com.example.new"),
            filteringScope = FilteringScope.All,
            excluded = setOf("com.example.old"),
        )
        val priorApps = setOf("com.example.old")
        val priorExclusions = setOf("com.example.notes")
        val engine = FakeAbd(apps = priorApps, excluded = priorExclusions).apply {
            failing += "scope all"
            beforeCommand = { command ->
                if (command == "scope all") assertEquals(FilterMethod.Root, fixture.userData.userData.value.method)
            }
        }

        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Apps)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))

        assertEquals(priorApps, engine.apps)
        assertEquals(priorExclusions, engine.excluded)
        assertEquals(FilteringScope.Chosen, engine.filteringScope)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
    }

    @Test
    fun givenVpnPolicyIsStaged_whenAnEditIsQueued_thenPublicationPrecedesRoutingThatEdit() = runTest {
        val root = FakeAbd(filteringScope = FilteringScope.All, excluded = setOf("com.example.bank"))
        val engine = FakeAbd()
        val repo = fixture.repository(root, engine)
        val client = fixture.routedClient(root, engine)
        val staging = CompletableDeferred<Unit>()
        val finishStaging = CompletableDeferred<Unit>()
        engine.beforeCommand = { command ->
            if (command == "scope all") {
                staging.complete(Unit)
                finishStaging.await()
            }
        }

        val switch = async { repo.switchTo(FilterMethod.Vpn) }
        staging.await()
        val edit = async { client.addApp("com.example.video") }
        runCurrent()
        assertEquals(root.excluded, engine.excluded)
        assertEquals(FilterMethod.Root, fixture.userData.userData.value.method)
        assertFalse(edit.isCompleted)

        finishStaging.complete(Unit)
        assertEquals(FilterMethodSwitch.Complete, switch.await())
        edit.await()
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(FilteringScope.All, engine.filteringScope)
        assertEquals(setOf("com.example.video"), engine.apps)
        assertTrue(root.apps.isEmpty())
    }

    @Test
    fun givenVpnSelectionRollbackCannotBeVerified_whenSwitchingFromRoot_thenVpnIsNotPublished() = runTest {
        val root = FakeAbd(apps = setOf("com.example.new"), excluded = setOf("com.example.bank"), filteringScope = FilteringScope.All)
        val engine = FakeAbd(apps = setOf("com.example.old")).apply {
            failing += setOf("scope all", "app add com.example.old")
        }

        assertFailsWith<AbdException.CommandFailed> { fixture.repository(root, engine).switchTo(FilterMethod.Vpn) }

        assertEquals(FilterMethod.Root, fixture.userData.userData.value.method)
        assertEquals(FilteringScope.Chosen, engine.filteringScope)
    }

    @Test
    fun aPartThatCannotBeReadIsLeftAsTheNewMethodHasIt() = runTest {
        val root = FakeAbd(apps = setOf("com.example.news"), rules = "||ads.example^\n").apply { failing += "rules get" }
        val engine = FakeAbd(rules = "||kept.example^\n")
        assertEquals(FilterMethodSwitch.VpnTransferIncomplete(setOf(VpnTransferPart.Rules)), fixture.repository(root, engine).switchTo(FilterMethod.Vpn))
        assertEquals("||kept.example^\n", engine.rules)
        assertEquals(setOf("com.example.news"), engine.apps)
    }

    @Test
    fun leavingTheVpnStopsIt() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd()
        val engine = FakeAbd(apps = setOf("com.example.news"), urlFiltering = false)
        fixture.repository(root, engine).switchTo(FilterMethod.Root)
        assertEquals(1, fixture.vpn.stops)
        assertEquals(FilterMethod.Root, fixture.userData.userData.value.method)
        assertEquals(setOf("com.example.news"), root.apps)
    }

    @Test
    fun choosingTheVpnDoesNotStartIt() = runTest {
        fixture.repository(FakeAbd(), FakeAbd()).switchTo(FilterMethod.Vpn)
        // FakeVpn.start fails the test if called; stop is for leaving it.
        assertEquals(0, fixture.vpn.stops)
    }

    @Test
    fun givenAllAppsAndExclusions_whenSwitchingMethods_thenScopeAndChoicesAreCarried() = runTest {
        val root = FakeAbd(filteringScope = FilteringScope.All, excluded = setOf("com.example.excluded"), apps = setOf("com.example.chosen"))
        val engine = FakeAbd()
        val repository = fixture.repository(root, engine)
        assertEquals(FilterMethodSwitch.Complete, repository.switchTo(FilterMethod.Vpn))
        assertEquals(FilteringScope.All, engine.filteringScope)
        assertEquals(root.excluded, engine.excluded)
        assertEquals(root.apps, engine.apps)

        engine.excluded += "com.example.new"
        assertEquals(FilterMethodSwitch.Complete, repository.switchTo(FilterMethod.Root))
        assertEquals(FilteringScope.All, root.filteringScope)
        assertEquals(engine.excluded, root.excluded)
        assertEquals(engine.apps, root.apps)
    }

    @Test
    fun givenRootUrlFilteringOn_whenSwitchingToVpn_thenThePreferenceIsCarried() = runTest {
        val engine = FakeAbd(urlFiltering = false)
        fixture.repository(FakeAbd(urlFiltering = true, cosmetic = true), engine).switchTo(FilterMethod.Vpn)
        assertTrue(engine.urlFiltering)
        assertTrue(engine.cosmetic)
    }

    @Test
    fun decryptionTurnedOffIsCarriedIntoTheVpn() = runTest {
        val engine = FakeAbd(urlFiltering = true, cosmetic = true)
        fixture.repository(FakeAbd(urlFiltering = false, cosmetic = false), engine).switchTo(FilterMethod.Vpn)
        assertFalse(engine.urlFiltering)
        // Hiding elements means nothing without decryption: left as it was.
        assertTrue(engine.cosmetic)
    }

    @Test
    fun givenVpnUrlFilteringOff_whenSwitchingToRoot_thenThePreferenceIsCarried() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd(urlFiltering = true, cosmetic = true)
        fixture.repository(root, FakeAbd(urlFiltering = false, cosmetic = false)).switchTo(FilterMethod.Root)
        assertFalse(root.urlFiltering)
        assertTrue(root.cosmetic)
    }

    @Test
    fun theSameMethodCarriesNothing() = runTest {
        val root = FakeAbd(apps = setOf("com.example.news"))
        fixture.repository(root, FakeAbd()).switchTo(FilterMethod.Root)
        assertEquals(emptyList(), root.ran)
    }

    @Test
    fun aRootSwitchSurvivesProcessDeathAndReplaysAfterInstall() = runTest {
        val directory = temporaryFolder.newFolder()
        val root = FakeAbd(apps = setOf("com.example.old"))
        val engine = FakeAbd(
            apps = setOf("com.example.news"),
            lists = mapOf(EASYLIST to true, CHINA to false),
            rules = "||ads.example^",
            trusted = listOf("bank.example"),
        )

        fixture.moduleState = ModuleState.NotInstalled
        fixture.withStoredPreferences(directory) { stored ->
            stored.setFilterMethod(FilterMethod.Vpn)
            assertEquals(FilterMethodSwitch.RootPending, fixture.repository(root, engine, stored).switchTo(FilterMethod.Root))
            assertTrue(stored.userData.first().rootSwitchPending)
            assertEquals(FilterMethod.Vpn, stored.userData.first().method)
            assertEquals(0, fixture.vpn.stops)
            assertTrue(root.ran.isEmpty())
        }

        // A choice made while the module was being installed is carried too.
        engine.apps += "com.example.video"
        fixture.withStoredPreferences(directory) { stored ->
            assertTrue(stored.userData.first().rootSwitchPending)
            val setup = PendingSetup(stored, fixture.module, unexpected(), unexpected(), fixture.repository(root, engine, stored), unexpected())
            fixture.moduleState = ModuleState.NeedsRestart
            setup.applyIfReady()
            assertTrue(stored.userData.first().rootSwitchPending)
            assertEquals(0, fixture.vpn.stops)

            fixture.moduleState = ModuleState.Ready
            setup.applyIfReady()
            assertEquals(FilterMethod.Root, stored.userData.first().method)
            assertFalse(stored.userData.first().rootSwitchPending)
            assertNull(stored.rootAppsBeforeSwitch())
            assertEquals(1, fixture.vpn.stops)
            assertEquals(engine.apps, root.apps)
            assertEquals(engine.lists, root.lists)
            assertEquals(engine.rules, root.rules)
            assertEquals(engine.trusted, root.trusted)
            assertEquals(1, fixture.rootUpdatesAsked)
            assertEquals(0, fixture.updatesAsked)
        }
    }

    @Test
    fun anUnreadableSourceCannotReplaceExistingRootSettings() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd(apps = setOf("com.example.old"), rules = "||kept.example^")
        val engine = FakeAbd(apps = setOf("com.example.news")).apply { failing += "rules get" }
        assertFailsWith<AbdException.CommandFailed> { fixture.repository(root, engine).switchTo(FilterMethod.Root) }
        assertTrue(fixture.userData.userData.value.rootSwitchPending)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(setOf("com.example.old"), root.apps)
        assertEquals("||kept.example^", root.rules)
        assertEquals(listOf("status --json"), root.ran)
        assertEquals(0, fixture.vpn.stops)
    }

    @Test
    fun aFailedListWriteLeavesTheVpnInUseAndRequestingRootAgainRetries() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd(lists = mapOf(CHINA to true), rules = "||kept.example^").apply { failing += "list add" }
        val engine = FakeAbd(lists = mapOf(EASYLIST to true), rules = "||ads.example^")
        val repo = fixture.repository(root, engine)
        assertFailsWith<AbdException.CommandFailed> { repo.switchTo(FilterMethod.Root) }
        assertEquals(mapOf(CHINA to true), root.lists)
        assertEquals("||kept.example^", root.rules)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertTrue(fixture.userData.userData.value.rootSwitchPending)
        assertEquals(0, fixture.vpn.stops)
        val attempt = checkNotNull(fixture.userData.rootHandoffAttempt())
        assertEquals(attempt, root.handoffToken)

        root.failing.clear()
        assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Root))
        assertEquals(listOf("handoff begin $attempt", "handoff begin $attempt", "handoff finish $attempt"), root.ran.filter { it.startsWith("handoff ") })
        assertEquals(engine.lists, root.lists)
        assertEquals(engine.rules, root.rules)
        assertEquals(1, fixture.vpn.stops)
        assertFalse(repo.retryRootSwitch())
    }

    @Test
    fun aModuleThatCannotRunKeepsTheVpnAndThePendingRequest() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd(running = false)
        val repo = fixture.repository(root, FakeAbd(apps = setOf("com.example.news")))
        assertEquals(FilterMethodSwitch.RootPending, repo.switchTo(FilterMethod.Root))
        assertTrue(fixture.userData.userData.value.rootSwitchPending)
        assertTrue(root.apps.isEmpty())
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(0, fixture.vpn.stops)
    }

    @Test
    fun cancellingARootSwitchKeepsTheVpnAndClearsTheRequest() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        fixture.moduleState = ModuleState.NotInstalled
        val root = FakeAbd().apply { unreachable = AbdException.NoRoot() }
        val repo = fixture.repository(root, FakeAbd(apps = setOf("com.example.news")))
        assertEquals(FilterMethodSwitch.RootPending, repo.switchTo(FilterMethod.Root))

        repo.cancelRootSwitch()
        fixture.moduleState = ModuleState.Ready
        assertFalse(repo.retryRootSwitch())
        assertFalse(fixture.userData.userData.value.rootSwitchPending)
        assertNull(fixture.userData.rootHandoffAttempt())
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertTrue(root.ran.isEmpty())
        assertEquals(0, fixture.vpn.stops)
    }

    @Test
    fun anOlderModuleThatRejectedTheFirstBeginCanBeCancelledWithoutRoot() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd()
        root.beforeCommand = { command ->
            if (command.startsWith("handoff begin ")) {
                assertEquals(command.removePrefix("handoff begin "), fixture.userData.rootHandoffAttempt())
                throw AbdException.CommandRejected()
            }
        }
        val repo = fixture.repository(root, FakeAbd(apps = setOf("com.example.news")))
        assertFailsWith<AbdException.CommandRejected> { repo.switchTo(FilterMethod.Root) }
        assertTrue(fixture.userData.userData.value.rootSwitchPending)
        assertNull(fixture.userData.rootHandoffAttempt())
        root.unreachable = AbdException.NoRoot()

        repo.cancelRootSwitch()
        assertFalse(fixture.userData.userData.value.rootSwitchPending)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(0, fixture.rootUpdatesAsked)
        assertEquals(0, fixture.vpn.stops)
    }

    @Test
    fun givenSnapshotAfterProcessDeath_whenCancelling_thenRootSettingsAndSelectionsAreRestored() = runTest {
        val directory = temporaryFolder.newFolder()
        val group = "@com.example.suite"
        val work = "37:com.example.work"
        val previous = setOf("com.example.bank", "com.example.old", group)
        val previousApps = previous + work
        val previousLists = mapOf(EASYLIST to false, CHINA to true)
        val previousRules = "||existing.example^\n\n"
        val previousTrusted = listOf("existing.example")
        val root = FakeAbd(apps = previousApps, lists = previousLists, rules = previousRules, trusted = previousTrusted)
        val engine = FakeAbd(
            apps = setOf("com.example.bank", "com.example.new"),
            lists = mapOf(EASYLIST to true),
            rules = "||vpn.example^\n",
            trusted = listOf("vpn.example"),
        )
        fixture.withStoredPreferences(directory) { stored ->
            stored.setFilterMethod(FilterMethod.Vpn)
            stored.setRootSwitchPending()
            val token = stored.startRootHandoff()
            root.run(listOf("handoff", "begin", token), label = "handoff begin")
            // The process ended after settings were replaced and activation began.
            stored.rememberRootAppsBeforeSwitch(previous)
            root.apps.retainAll(setOf(group, work))
            root.apps.addAll(engine.apps)
            root.lists.clear()
            root.lists.putAll(engine.lists)
            root.rules = engine.rules
            root.trusted.clear()
            root.trusted.addAll(engine.trusted)
        }
        fixture.withStoredPreferences(directory) { stored ->
            assertEquals(previous, stored.rootAppsBeforeSwitch())
            val token = checkNotNull(stored.rootHandoffAttempt())
            root.beforeCommand = { if (it == "handoff rollback $token") assertEquals(previousApps, root.apps) }
            fixture.repository(root, engine, stored).cancelRootSwitch()
            assertEquals(previousApps, root.apps)
            assertTrue(root.ran.none { it == "app add $group" || it == "app remove $group" || work in it })
            assertEquals(previousLists, root.lists)
            assertEquals(previousRules, root.rules)
            assertEquals(previousTrusted, root.trusted)
            assertEquals(FilterMethod.Vpn, stored.userData.first().method)
            assertFalse(stored.userData.first().rootSwitchPending)
            assertFalse(stored.userData.first().rootSwitchCancelling)
            assertNull(stored.rootAppsBeforeSwitch())
            assertNull(stored.rootHandoffAttempt())
            assertNull(root.handoffToken)
            assertEquals(1, fixture.rootUpdatesAsked)
            assertEquals(0, fixture.vpn.stops)
        }
    }

    @Test
    fun aFailedCancellationAfterAnIncompleteWriteRetriesTheRollbackOnLaunch() = runTest {
        val directory = temporaryFolder.newFolder()
        val root = FakeAbd(lists = mapOf(CHINA to true), rules = "||root.example^", trusted = listOf("root.example")).apply {
            failing += setOf("app add com.example.retry", "app remove com.example.video")
        }
        val engine = FakeAbd(
            apps = setOf("com.example.video", "com.example.retry"),
            lists = mapOf(EASYLIST to true),
            rules = "||vpn.example^",
            trusted = listOf("vpn.example"),
        )
        fixture.withStoredPreferences(directory) { stored ->
            stored.setFilterMethod(FilterMethod.Vpn)
            val repo = fixture.repository(root, engine, stored)
            assertFailsWith<AbdException.CommandFailed> { repo.switchTo(FilterMethod.Root) }
            assertEquals(setOf("com.example.video"), root.apps)
            assertEquals(emptySet(), stored.rootAppsBeforeSwitch())
            assertEquals(engine.lists, root.lists)
            assertEquals(engine.rules, root.rules)
            assertEquals(engine.trusted, root.trusted)
            assertEquals(root.handoffToken, stored.rootHandoffAttempt())
        }

        fixture.withStoredPreferences(directory) { stored ->
            assertFailsWith<AbdException.CommandFailed> { fixture.repository(root, engine, stored).cancelRootSwitch() }
            assertEquals(FilterMethod.Vpn, stored.userData.first().method)
            assertTrue(stored.userData.first().rootSwitchPending)
            assertTrue(stored.userData.first().rootSwitchCancelling)
            assertEquals(emptySet(), stored.rootAppsBeforeSwitch())
            assertTrue(root.ran.none { it.startsWith("handoff rollback") })
        }

        root.failing.clear()
        root.beforeCommand = { if (it.startsWith("handoff rollback ")) throw AbdException.CommandRejected() }
        fixture.withStoredPreferences(directory) { stored ->
            assertFailsWith<AbdException.CommandRejected> { fixture.repository(root, engine, stored).retryRootSwitch() }
            assertEquals(emptySet(), root.apps)
            assertTrue(stored.userData.first().rootSwitchCancelling)
            assertEquals(root.handoffToken, stored.rootHandoffAttempt())
            assertEquals(0, fixture.rootUpdatesAsked)
        }
        root.beforeCommand = {}
        fixture.withStoredPreferences(directory) { stored ->
            PendingSetup(stored, fixture.module, unexpected(), unexpected(), fixture.repository(root, engine, stored), unexpected()).applyIfReady()
            assertEquals(emptySet(), root.apps)
            assertEquals(mapOf(CHINA to true), root.lists)
            assertEquals("||root.example^", root.rules)
            assertEquals(listOf("root.example"), root.trusted)
            assertEquals(FilterMethod.Vpn, stored.userData.first().method)
            assertFalse(stored.userData.first().rootSwitchPending)
            assertFalse(stored.userData.first().rootSwitchCancelling)
            assertNull(stored.rootAppsBeforeSwitch())
            assertNull(stored.rootHandoffAttempt())
            assertEquals(1, fixture.rootUpdatesAsked)
            assertEquals(0, fixture.vpn.stops)
        }
    }

    @Test
    fun aPublishedHandoffFinishesEvenIfItsRootSnapshotWasAlreadyRemoved() = runTest {
        val directory = temporaryFolder.newFolder()
        val root = FakeAbd(rules = "||old.example^")
        val engine = FakeAbd(rules = "||new.example^")
        fixture.rootUpdateFailed = true
        fixture.withStoredPreferences(directory) { stored ->
            stored.setFilterMethod(FilterMethod.Vpn)
            val repo = fixture.repository(root, engine, stored)
            assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Root))
            assertEquals(FilterMethod.Root, stored.userData.first().method)
            assertTrue(stored.userData.first().rootHandoffCleanupPending)
            assertNull(stored.rootHandoffAttempt())
            assertNull(root.handoffToken)
            assertEquals(engine.rules, root.rules)
            assertEquals(0, fixture.rootUpdatesAsked)
        }
        fixture.rootUpdateFailed = false
        fixture.withStoredPreferences(directory) { stored ->
            val token = checkNotNull(stored.rootHandoffToFinish())
            val repo = fixture.repository(root, engine, stored)
            repo.cancelRootSwitch()
            PendingSetup(stored, fixture.module, unexpected(), unexpected(), repo, unexpected()).applyIfReady()
            assertEquals(FilterMethod.Root, stored.userData.first().method)
            assertFalse(stored.userData.first().rootHandoffCleanupPending)
            assertEquals(listOf("handoff begin $token", "handoff finish $token", "handoff finish $token"), root.ran.filter { it.startsWith("handoff ") })
            assertEquals(engine.rules, root.rules)
            assertEquals(1, fixture.rootUpdatesAsked)
        }
    }

    @Test
    fun cancellingANewAttemptRestoresTheLastPublishedConfiguration() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd(rules = "||original.example^").apply { failing += "handoff finish" }
        val engine = FakeAbd(rules = "||committed.example^", lists = mapOf(CHINA to true), trusted = listOf("committed.example"))
        val repo = fixture.repository(root, engine)
        assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Root))
        val committed = checkNotNull(fixture.userData.rootHandoffToFinish())
        assertEquals(committed, root.handoffToken)

        assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Vpn))
        engine.rules = "||next.example^"
        engine.lists.clear()
        engine.lists[EASYLIST] = true
        engine.trusted.clear()
        engine.trusted += "next.example"
        assertEquals(FilterMethodSwitch.RootPending, repo.switchTo(FilterMethod.Root))
        assertNull(fixture.userData.rootHandoffAttempt())
        assertEquals(committed, root.handoffToken)

        root.failing.clear()
        root.beforeCommand = { if (it == "app add com.example.new") throw AbdException.CommandFailed("app write failed") }
        engine.apps += "com.example.new"
        assertFailsWith<AbdException.CommandFailed> { repo.retryRootSwitch() }
        val newToken = checkNotNull(fixture.userData.rootHandoffAttempt())
        assertTrue(newToken != committed)
        assertEquals(engine.rules, root.rules)

        repo.cancelRootSwitch()
        assertEquals("||committed.example^", root.rules)
        assertEquals(mapOf(CHINA to true), root.lists)
        assertEquals(listOf("committed.example"), root.trusted)
        assertFalse(fixture.userData.userData.value.rootSwitchPending)
        assertNull(root.handoffToken)
        assertEquals(2, fixture.rootUpdatesAsked)
    }

    @Test
    fun theSwitchWaitsForAnEditAlreadyRoutedToTheVpn() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd()
        val engine = FakeAbd(apps = setOf("com.example.news"))
        val repo = fixture.repository(root, engine)
        val client = fixture.routedClient(root, engine)
        val editStarted = CompletableDeferred<Unit>()
        val finishEdit = CompletableDeferred<Unit>()
        engine.beforeCommand = {
            if (it == "rules append") {
                editStarted.complete(Unit)
                finishEdit.await()
            }
        }

        val edit = async { client.appendUserRules("||ads.example^\n") }
        editStarted.await()
        val switch = async { repo.switchTo(FilterMethod.Root) }
        runCurrent()
        assertFalse(switch.isCompleted)
        assertTrue(root.apps.isEmpty())
        assertEquals("com.example.news", client.apps().single().packageName)

        finishEdit.complete(Unit)
        edit.await()
        assertEquals(FilterMethodSwitch.RootPending, switch.await())
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(0, fixture.vpn.stops)

        assertTrue(repo.retryRootSwitch())
        assertEquals("||ads.example^\n", root.rules)
    }

    @Test
    fun anEditQueuedDuringPublicationUsesTheNewMethod() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd()
        val engine = FakeAbd()
        val repo = fixture.repository(root, engine)
        val client = fixture.routedClient(root, engine)
        val verifying = CompletableDeferred<Unit>()
        val finishVerification = CompletableDeferred<Unit>()
        var reads = 0
        engine.beforeCommand = {
            if (it == "allow list --json" && ++reads == 2) {
                verifying.complete(Unit)
                finishVerification.await()
            }
        }

        val switch = async { repo.switchTo(FilterMethod.Root) }
        verifying.await()
        val edit = async { client.addApp("com.example.video") }
        runCurrent()
        assertFalse(edit.isCompleted)

        finishVerification.complete(Unit)
        assertEquals(FilterMethodSwitch.Complete, switch.await())
        edit.await()
        assertTrue(engine.apps.isEmpty())
        assertEquals(setOf("com.example.video"), root.apps)
    }

    @Test
    fun givenAnUnavailableRootSelection_whenSwitchingFromVpn_thenItsSavedModeIsKept() = runTest {
        val root = FakeAbd(apps = setOf("com.example.shared")).apply { shared += "com.example.shared" }
        val engine = FakeAbd(apps = setOf("com.example.news"))
        fixture.userData.setFilterMethod(FilterMethod.Vpn)

        assertEquals(FilterMethodSwitch.Complete, fixture.repository(root, engine).switchTo(FilterMethod.Root))
        assertEquals(setOf("com.example.shared", "com.example.news"), root.apps)
        assertTrue(root.ran.none { it.startsWith("app remove com.example.shared") })
    }

    @Test
    fun givenRootGroupAndVpnMember_whenSwitchingMethods_thenTheGroupIsNeverCarriedOrChanged() = runTest {
        val group = "@com.example.suite"
        val member = "com.example.news"
        val root = FakeAbd(apps = setOf(group)).apply {
            eligibility = """{"apps":{"$member":"shared_uid"},"groups":[{"id":"com.example.suite","packages":["$member"]}]}"""
        }
        val engine = FakeAbd()
        val repo = fixture.repository(root, engine)
        assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Vpn))
        assertTrue(engine.ran.none { group in it })
        assertTrue(engine.apps.isEmpty())

        engine.apps += member
        assertEquals(FilterMethodSwitch.Complete, repo.switchTo(FilterMethod.Root))
        assertEquals(setOf(group), root.apps)
        assertTrue(root.ran.none { it.startsWith("app add $member") })
    }

    @Test
    fun aFailedRootAppWriteRestoresThePriorSelection() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val previous = setOf("com.example.old", "com.example.news")
        val root = FakeAbd(apps = previous).apply { failing += "app add com.example.retry" }
        val engine = FakeAbd(
            apps = setOf("com.example.news", "com.example.video", "com.example.retry"),
        )
        val repo = fixture.repository(root, engine)
        assertFailsWith<AbdException.CommandFailed> { repo.switchTo(FilterMethod.Root) }
        assertEquals(previous, root.apps)
        assertTrue(fixture.userData.userData.value.rootSwitchPending)
        assertEquals(FilterMethod.Vpn, fixture.userData.userData.value.method)
        assertEquals(0, fixture.vpn.stops)

        repo.cancelRootSwitch()
        assertEquals(previous, root.apps)
        assertFalse(fixture.userData.userData.value.rootSwitchPending)
    }

    @Test
    fun cancellationDuringRootActivationLetsTheHandoffFinish() = runTest {
        fixture.userData.setFilterMethod(FilterMethod.Vpn)
        val root = FakeAbd()
        val engine = FakeAbd(apps = setOf("com.example.news"))
        val appWriteStarted = CompletableDeferred<Unit>()
        val finishAppWrite = CompletableDeferred<Unit>()
        root.beforeCommand = {
            if (it == "app add com.example.news") {
                appWriteStarted.complete(Unit)
                finishAppWrite.await()
            }
        }

        val switch = async { fixture.repository(root, engine).switchTo(FilterMethod.Root) }
        appWriteStarted.await()
        switch.cancel()
        finishAppWrite.complete(Unit)
        switch.join()
        assertEquals(engine.apps, root.apps)
        assertEquals(FilterMethod.Root, fixture.userData.userData.value.method)
        assertFalse(fixture.userData.userData.value.rootSwitchPending)
    }

    private companion object {
        const val EASYLIST = "https://easylist.to/easylist/easylist.txt"
        const val CHINA = "https://easylist-downloads.adblockplus.org/easylistchina.txt"
    }
}
