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

import android.content.Intent
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.job
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.AbdWriteLock
import li.mercury.adblocker.core.data.DefaultAbdClient
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.data.vpn.VpnController
import li.mercury.adblocker.core.datastore.UserPreferencesDataSource
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.FilterMethod
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.ModuleState
import li.mercury.adblocker.core.model.UserData
import li.mercury.adblocker.core.model.VpnStatus
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import java.io.File
import java.util.UUID

internal class FilterMethodRepositoryFixture {
    /**
     * An abd of one method, holding its state in memory and printing it as
     * the daemon does (testdata/protocol). Commands in [failing], by their
     * first words, exit with an error; with [unreachable], every one does,
     * as without root.
     */
    class FakeAbd(
        apps: Set<String> = emptySet(),
        lists: Map<String, Boolean> = emptyMap(),
        trusted: List<String> = emptyList(),
        var rules: String = "",
        var urlFiltering: Boolean = true,
        var cosmetic: Boolean = false,
        var running: Boolean = true,
        var filteringScope: FilteringScope = FilteringScope.Chosen,
        var excluded: Set<String> = emptySet(),
    ) : AbdTransport {
        private data class Snapshot(
            val token: String,
            val lists: Map<String, Boolean>,
            val rules: String,
            val trusted: List<String>,
            val urlFiltering: Boolean,
            val filteringScope: FilteringScope,
            val excluded: Set<String>,
        )

        val apps = apps.toMutableSet()
        val lists = lists.toMutableMap()
        val trusted = trusted.toMutableList()
        val failing = mutableSetOf<String>()
        val shared = mutableSetOf<String>()
        var eligibility = """{"apps":{},"groups":[]}"""
        var unreachable: AbdException? = null
        val ran = mutableListOf<String>()
        var beforeCommand: suspend (String) -> Unit = {}
        private var snapshot: Snapshot? = null
        val handoffToken: String? get() = snapshot?.token

        override suspend fun run(args: List<String>, label: String, input: String?): String {
            unreachable?.let { throw it }
            val line = args.joinToString(" ")
            beforeCommand(line)
            if (failing.any { line.startsWith(it) }) throw AbdException.CommandFailed("failed")
            ran += line
            return when {
                line == "app eligibility --json" -> eligibility

                line == "app selections --json" -> {
                    val chosen = apps.joinToString(",", "[", "]") { "\"$it\"" }
                    val deny = excluded.joinToString(",", "[", "]") { "\"$it\"" }
                    """{"scope":"${filteringScope.name.lowercase()}","chosen":$chosen,"excluded":$deny}"""
                }

                line == "app configure-exclusions" -> "".also { excluded = input.orEmpty().lineSequence().filter(String::isNotBlank).toSet() }

                args[0] == "scope" -> "".also { filteringScope = if (args[1] == "all") FilteringScope.All else FilteringScope.Chosen }

                line == "app list --json" -> apps.joinToString(",", "[", "]") { p ->
                    val identity = AppIdentity.fromKey(p)
                    val profile = identity.profileSerial?.let { ""","profile_serial":$it""" }.orEmpty()
                    val isShared = p in shared
                    val unavailable = if (isShared) ""","unavailable_reason":"shared_uid"""" else ""
                    val mode = if (isShared) {
                        "off"
                    } else if (urlFiltering) {
                        "full"
                    } else {
                        "hosts"
                    }
                    val reason = if (mode == "hosts") "\"url_filtering_off\"" else "null"
                    val level = """{"level":"$mode","reason":$reason}"""
                    """{"package":"${identity.packageName}"$profile,"effective":{"http":$level,"https":$level},"paused_until":null$unavailable}"""
                }

                line == "lists --json" -> lists.entries.joinToString(",", "[", "]") { (url, on) -> """{"url":"$url","enabled":$on}""" }

                line == "allow list --json" -> trusted.joinToString(",", "[", "]") { "\"$it\"" }

                line == "rules get" -> rules

                line == "status --json" -> """{"running":$running,"paused":false,"url_filtering":$urlFiltering,"cosmetic":$cosmetic}"""

                line == "rules set" -> "".also { rules = input.orEmpty() }

                line == "rules append" -> "".also { rules = rules + input.orEmpty() }

                args[0] == "url-filtering" -> "".also { urlFiltering = args[1] == "on" }

                args[0] == "cosmetic" -> "".also { cosmetic = args[1] == "on" }

                args.take(2) == listOf("app", "add") -> "".also {
                    apps += args[2]
                }

                args.take(2) == listOf("app", "remove") -> "".also { apps.remove(args[2]) }

                args.take(2) == listOf("allow", "add") -> "".also { if (args[2] !in trusted) trusted += args[2] }

                args.take(2) == listOf("allow", "remove") -> "".also { trusted.remove(args[2]) }

                args.take(2) == listOf("list", "add") -> "".also { lists[args[2]] = true }

                args.take(2) == listOf("list", "remove") -> "".also { lists.remove(args[2]) }

                args.take(2) == listOf("list", "enable") -> "".also { lists[args[2]] = true }

                args.take(2) == listOf("list", "disable") -> "".also { lists[args[2]] = false }

                args[0] == "handoff" -> handoff(args[1], args[2])

                else -> error("not expected: $line")
            }
        }

        private fun handoff(action: String, token: String): String {
            require(UUID.fromString(token).toString() == token)
            val saved = snapshot
            if (action == "begin") {
                if (saved?.token != token) snapshot = Snapshot(token, lists.toMap(), rules, trusted.toList(), urlFiltering, filteringScope, excluded)
                return ""
            }
            if (saved == null) return ""
            if (saved.token != token) throw AbdException.CommandFailed("wrong token")
            if (action == "rollback") {
                lists.clear()
                lists.putAll(saved.lists)
                urlFiltering = saved.urlFiltering
                filteringScope = saved.filteringScope
                excluded = saved.excluded
                rules = saved.rules
                trusted.clear()
                trusted.addAll(saved.trusted)
            } else {
                check(action == "finish")
            }
            snapshot = null
            return ""
        }

        override fun stream(args: List<String>, label: String, endsWithInput: Boolean): Flow<String> = error("not expected")
    }

    class FakeVpn : VpnController {
        override suspend fun engineAvailable() = true
        var stops = 0
        override val status: StateFlow<VpnStatus> = MutableStateFlow(VpnStatus())

        override fun consentIntent(): Intent? = null

        override suspend fun start(): Unit = error("the screens start the VPN, after consent")

        override suspend fun stop() {
            stops++
        }
    }

    val userData = TestUserDataRepository(UserData(filterMethod = FilterMethod.Root))
    val vpn = FakeVpn()
    private val writes = AbdWriteLock()
    var moduleState = ModuleState.Ready
    val module: ModuleRepository = object : ModuleRepository by unexpected() {
        override suspend fun state() = moduleState

        override suspend fun startDaemon() = false
    }
    var updatesAsked = 0
        private set
    var rootUpdatesAsked = 0
        private set
    var rootUpdateFailed = false

    fun repository(root: FakeAbd, engine: FakeAbd, preferences: UserDataRepository = userData) = DefaultFilterMethodRepository(
        rootShell = RootShell(Dispatchers.Unconfined),
        userData = preferences,
        transportOf = { if (it == FilterMethod.Root) root else engine },
        vpn = vpn,
        module = module,
        changes = DataChanges(),
        updates = object : ListUpdateScheduler {
            override suspend fun updateSoon() {
                updatesAsked++
            }

            override suspend fun updateRootSoon() {
                if (rootUpdateFailed) throw IllegalStateException("could not enqueue the work")
                rootUpdatesAsked++
            }
        },
        writes = writes,
    )

    fun routedClient(root: FakeAbd, engine: FakeAbd) = DefaultAbdClient(
        object : TransportSource {
            override suspend fun current() = if (userData.userData.value.method == FilterMethod.Root) root else engine
        },
        writes,
    )

    suspend fun withStoredPreferences(directory: File, action: suspend (UserDataRepository) -> Unit) {
        val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
        try {
            action(
                DataStoreUserDataRepository(
                    UserPreferencesDataSource(PreferenceDataStoreFactory.create(scope = scope) { directory.resolve("user.preferences_pb") }),
                ),
            )
        } finally {
            scope.coroutineContext.job.cancelAndJoin()
        }
    }
}
