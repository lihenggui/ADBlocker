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

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import li.mercury.adblocker.core.data.protocol.AppEligibility
import li.mercury.adblocker.core.data.protocol.WorkProfile
import li.mercury.adblocker.core.data.protocol.parseAllowList
import li.mercury.adblocker.core.data.protocol.parseAppEligibility
import li.mercury.adblocker.core.data.protocol.parseAppSelections
import li.mercury.adblocker.core.data.protocol.parseApps
import li.mercury.adblocker.core.data.protocol.parseCheck
import li.mercury.adblocker.core.data.protocol.parseLists
import li.mercury.adblocker.core.data.protocol.parseOverview
import li.mercury.adblocker.core.data.protocol.parsePinned
import li.mercury.adblocker.core.data.protocol.parseRuleEdit
import li.mercury.adblocker.core.data.protocol.parseRulesCheck
import li.mercury.adblocker.core.data.protocol.parseStats
import li.mercury.adblocker.core.data.protocol.parseStatus
import li.mercury.adblocker.core.data.protocol.parseUpdateEvent
import li.mercury.adblocker.core.data.protocol.parseWatchLine
import li.mercury.adblocker.core.data.protocol.parseWorkProfiles
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.EngineTransport
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.DailyStats
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.WatchEvent
import li.mercury.adblocker.core.model.WorkSelection
import java.util.concurrent.atomic.AtomicLong
import javax.inject.Inject
import javax.inject.Singleton

data class AbdOverview(val status: DaemonStatus, val stats: DailyStats, val lists: List<FilterList>)

/**
 * The daemon's commands. Every read and every change of the daemon's state
 * goes through here: the app never touches /data/adb/adblocker itself.
 */
interface AbdClient {
    suspend fun status(): DaemonStatus

    suspend fun apps(): List<FilteredApp>

    suspend fun appSelections(): AppSelections

    suspend fun setFilteringScope(scope: FilteringScope)

    suspend fun setAppExcluded(packageName: String, excluded: Boolean)

    suspend fun configureExclusions(apps: Set<String>)

    /** The root module's eligibility report for personal apps. Null values are eligible. */
    suspend fun appEligibility(): AppEligibility

    /** Managed profiles supported by the root module. */
    suspend fun workProfiles(): List<WorkProfile> = emptyList()

    /** Counts between each pair of successive [dayStarts], in epoch seconds. */
    suspend fun stats(dayStarts: List<Long>): DailyStats

    /** Status, daily counts for [dayStarts] as above, and the subscribed lists. */
    suspend fun overview(dayStarts: List<Long>): AbdOverview

    suspend fun lists(): List<FilterList>

    suspend fun pinned(packageName: String? = null): List<PinnedHost>

    suspend fun allowList(): List<String>

    suspend fun userRules(): String

    suspend fun checkRules(text: String): RulesCheck

    /** How the rules treat a request; with [app], as `watch` names it, the rules for that app are considered too. */
    suspend fun check(url: String, page: String?, kind: String?, app: String? = null): RuleCheckResult

    suspend fun checkHost(host: String, app: String): RuleCheckResult

    suspend fun setUserRules(text: String)

    suspend fun appendUserRules(text: String, unique: Boolean = false)

    /** Whether a previously absent line was appended. */
    suspend fun appendUniqueUserRule(text: String): Boolean

    /** Whether one or more exact matching lines were removed. */
    suspend fun removeUserRules(text: String): Boolean

    /** Pauses filtering of every app, for [minutes], or until [resume] if null. */
    suspend fun pause(minutes: Int?)

    suspend fun resume()

    /** Turns decryption on or off for every app; the daemon reloads. */
    suspend fun setUrlFiltering(on: Boolean)

    /** Turns the hiding of the elements the lists name on decrypted pages on or off. */
    suspend fun setCosmetic(on: Boolean)

    /** Filters [packageName]. How deeply follows the global URL filtering setting. */
    suspend fun addApp(packageName: String)

    suspend fun configureApps(packages: Set<String>, work: WorkSelection = WorkSelection.Replace)

    suspend fun removeApp(packageName: String)

    suspend fun pauseApp(packageName: String, minutes: Int)

    suspend fun resumeApp(packageName: String)

    suspend fun allow(host: String)

    suspend fun disallow(host: String)

    suspend fun addList(url: String)

    suspend fun configureLists(choices: Map<String, Boolean>)

    suspend fun removeList(url: String)

    suspend fun setListEnabled(url: String, enabled: Boolean)

    suspend fun forgetPinned(packageName: String, host: String?)

    /** Downloads the lists, reporting as it goes. Stopping the collection stops the download. */
    fun update(): Flow<UpdateEvent>

    /** Downloads a single enabled filter list. */
    fun updateList(url: String): Flow<UpdateEvent>

    /**
     * What happens to the filtered apps' connections, as it happens, for as
     * long as it is collected. Only [packageName]'s if given.
     */
    fun watch(packageName: String?): Flow<WatchEvent>
}

/** Keeps command routing fixed until a mutation, or a method handoff, is complete. */
@Singleton
internal class AbdWriteLock @Inject constructor() {
    val mutex = Mutex()
}

@Singleton
internal class DefaultAbdClient @Inject constructor(
    private val transports: TransportSource,
    // A fixed transport client is used by the handoff while it holds the
    // routed client's lock. It owns a separate lock.
    private val writes: AbdWriteLock = AbdWriteLock(),
) : AbdClient {
    private val nextEventId = AtomicLong()

    private suspend fun abd(vararg args: String, label: String = args.take(2).joinToString(" "), input: String? = null): String = transports.current().run(args.toList(), label, input)

    private suspend fun write(vararg args: String, label: String, input: String? = null) {
        writes.mutex.withLock { abd(*args, label = label, input = input) }
    }

    override suspend fun status() = parseStatus(abd("status", "--json", label = "status"))

    override suspend fun appSelections() = parseAppSelections(abd("app", "selections", "--json", label = "app selections"))

    override suspend fun setFilteringScope(scope: FilteringScope) = write("scope", scope.name.lowercase(), label = "scope")

    override suspend fun setAppExcluded(packageName: String, excluded: Boolean) = write("app", if (excluded) "exclude" else "include", packageName, label = "app exclusion")

    override suspend fun configureExclusions(apps: Set<String>) = write("app", "configure-exclusions", label = "app configure-exclusions", input = apps.sorted().joinToString("\n"))

    override suspend fun apps() = parseApps(abd("app", "list", "--json", label = "app list"))

    override suspend fun appEligibility(): AppEligibility {
        val transport = transports.current()
        if (transport is EngineTransport) throw AbdException.CommandFailed("App eligibility requires root")
        return parseAppEligibility(transport.run(listOf("app", "eligibility", "--json"), label = "app eligibility"))
    }

    override suspend fun workProfiles(): List<WorkProfile> {
        val root = (transports.current() as? RootTransport) ?: return emptyList()
        return parseWorkProfiles(root.run(listOf("app", "profiles", "--json"), label = "app profiles"))
    }

    override suspend fun stats(dayStarts: List<Long>): DailyStats {
        val args = listOf("stats", "--json") + dayStarts.map(Long::toString)
        return parseStats(transports.current().run(args, label = "stats"))
    }

    override suspend fun overview(dayStarts: List<Long>): AbdOverview {
        val args = listOf("overview", "--json") + dayStarts.map(Long::toString)
        return parseOverview(transports.current().run(args, label = "overview"))
    }

    override suspend fun lists() = parseLists(abd("lists", "--json", label = "lists"))

    override suspend fun pinned(packageName: String?) = parsePinned(abd(*(listOf("pinned", "--json") + listOfNotNull(packageName)).toTypedArray(), label = "pinned"))

    override suspend fun allowList() = parseAllowList(abd("allow", "list", "--json", label = "allow list"))

    override suspend fun userRules() = abd("rules", "get", label = "rules get")

    override suspend fun checkRules(text: String) = parseRulesCheck(abd("rules", "check", "--json", label = "rules check", input = text))

    override suspend fun check(url: String, page: String?, kind: String?, app: String?): RuleCheckResult {
        val args = buildList {
            add("check")
            add("--json")
            if (app != null) {
                add("--app")
                add(app)
            }
            add(url)
            if (page != null || kind != null) add(page.orEmpty())
            if (kind != null) add(kind)
        }
        return parseCheck(abd(*args.toTypedArray(), label = "check"))
    }

    override suspend fun checkHost(host: String, app: String): RuleCheckResult = parseCheck(abd("check", "--json", "--app", app, "--host", host, label = "check"))

    override suspend fun setUserRules(text: String) = write("rules", "set", label = "rules set", input = text)

    override suspend fun appendUserRules(text: String, unique: Boolean) {
        val args = if (unique) arrayOf("rules", "append", "--unique") else arrayOf("rules", "append")
        write(*args, label = "rules append", input = text)
    }

    override suspend fun appendUniqueUserRule(text: String): Boolean = writes.mutex.withLock {
        val transport = transports.current()
        val args = listOf("rules", "append", "--unique")
        try {
            parseRuleEdit(transport.run(args + "--json", label = "rules append", input = text))
        } catch (_: AbdException.ModuleOutdated) {
            // An older root daemon can still add the rule but cannot confirm that it was new.
            transport.run(args, label = "rules append", input = text)
            false
        }
    }

    override suspend fun removeUserRules(text: String): Boolean = writes.mutex.withLock {
        parseRuleEdit(abd("rules", "remove", "--json", label = "rules remove", input = text))
    }

    override suspend fun setUrlFiltering(on: Boolean) = write("url-filtering", if (on) "on" else "off", label = "url-filtering")

    override suspend fun setCosmetic(on: Boolean) = write("cosmetic", if (on) "on" else "off", label = "cosmetic")

    override suspend fun pause(minutes: Int?) = write(*(listOf("pause") + (minutes?.let { listOf("--minutes", "$it") } ?: emptyList())).toTypedArray(), label = "pause")

    override suspend fun resume() = write("resume", label = "resume")

    override suspend fun addApp(packageName: String) = write("app", "add", packageName, label = "app add")

    override suspend fun configureApps(packages: Set<String>, work: WorkSelection) {
        val args = if (work == WorkSelection.Keep) arrayOf("app", "configure", "--keep-work") else arrayOf("app", "configure")
        write(*args, label = "app configure", input = packages.joinToString("\n"))
    }

    override suspend fun removeApp(packageName: String) = write("app", "remove", packageName, label = "app remove")

    override suspend fun pauseApp(packageName: String, minutes: Int) = write("app", "pause", packageName, "--minutes", "$minutes", label = "app pause")

    override suspend fun resumeApp(packageName: String) = write("app", "resume", packageName, label = "app resume")

    override suspend fun allow(host: String) = write("allow", "add", host, label = "allow add")

    override suspend fun disallow(host: String) = write("allow", "remove", host, label = "allow remove")

    override suspend fun addList(url: String) = write("list", "add", url, label = "list add")

    override suspend fun configureLists(choices: Map<String, Boolean>) {
        val selection = choices.entries.joinToString("\n") { (url, enabled) -> if (enabled) url else "#$url" }
        write("list", "configure", input = selection, label = "list configure")
    }

    override suspend fun removeList(url: String) = write("list", "remove", url, label = "list remove")

    override suspend fun setListEnabled(url: String, enabled: Boolean) = write("list", if (enabled) "enable" else "disable", url, label = "list enable")

    override suspend fun forgetPinned(packageName: String, host: String?) = write(*(listOf("pinned", "forget", packageName) + listOfNotNull(host)).toTypedArray(), label = "pinned forget")

    override fun update(): Flow<UpdateEvent> = streamUpdate(listOf("update", "--json"))

    override fun updateList(url: String): Flow<UpdateEvent> = streamUpdate(listOf("update", "--json", "--list", url))

    private fun streamUpdate(args: List<String>): Flow<UpdateEvent> = flow {
        emitAll(transports.current().stream(args, "update", endsWithInput = false))
    }.mapNotNull { line -> line.takeIf { it.isNotBlank() }?.let(::parseUpdateEvent) }

    @OptIn(ExperimentalCoroutinesApi::class)
    override fun watch(packageName: String?): Flow<WatchEvent> {
        val args = listOf("watch", "--json") + (packageName?.let { listOf("--app", it) } ?: emptyList())
        return transports.observe().flatMapLatest { it.stream(args, "watch", endsWithInput = true) }
            .mapNotNull { line -> line.takeIf { it.isNotBlank() }?.let { parseWatchLine(it, nextEventId.incrementAndGet()) } }
    }
}

/** A client bound to one filtering method even while the active method changes. */
internal fun AbdTransport.fixedAbdClient(): AbdClient = DefaultAbdClient(
    object : TransportSource {
        override suspend fun current() = this@fixedAbdClient
    },
)
