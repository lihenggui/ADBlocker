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

package li.mercury.adblocker.core.data.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.AbdOverview
import li.mercury.adblocker.core.model.AppIdentity
import li.mercury.adblocker.core.model.AppSelections
import li.mercury.adblocker.core.model.AppUnavailableReason
import li.mercury.adblocker.core.model.BlockedCount
import li.mercury.adblocker.core.model.DaemonStatus
import li.mercury.adblocker.core.model.DailyStats
import li.mercury.adblocker.core.model.FilterLevel
import li.mercury.adblocker.core.model.FilterList
import li.mercury.adblocker.core.model.FilteredApp
import li.mercury.adblocker.core.model.FilteringScope
import li.mercury.adblocker.core.model.LevelReason
import li.mercury.adblocker.core.model.ListUpdateState
import li.mercury.adblocker.core.model.PassReason
import li.mercury.adblocker.core.model.PinnedHost
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.RuleCheckResult
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.model.RuleSource
import li.mercury.adblocker.core.model.RulesCheck
import li.mercury.adblocker.core.model.UpdateEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.model.WatchEvent
import java.time.Instant

/*
 * What abd prints, read into the model. The formats are fixed by the files in
 * testdata/protocol, which the daemon's tests and AbdProtocolTest both read.
 * Unknown fields are ignored. A missing required field fails with
 * AbdException.BadOutput.
 */

private val json = Json {
    ignoreUnknownKeys = true
    // Values of enum-like fields the app does not know are mapped by hand.
    coerceInputValues = false
}

// The serializer's messages quote the text they could not read, which can
// name hosts and addresses: only the kind of error is kept.
private inline fun <T> reading(what: String, block: () -> T): T = try {
    block()
} catch (e: SerializationException) {
    throw AbdException.BadOutput("$what: ${e.javaClass.simpleName}")
} catch (e: IllegalArgumentException) {
    throw AbdException.BadOutput("$what: ${e.javaClass.simpleName}")
}

private fun Long?.toInstant(): Instant? = this?.let(Instant::ofEpochSecond)

@Serializable
private class StatusDto(
    val running: Boolean,
    val pid: Int? = null,
    val started: Long? = null,
    val version: String? = null,
    val apps: Int? = null,
    @SerialName("filtering_apps") val filteringApps: Int? = null,
    val rules: Int? = null,
    @SerialName("host_rules") val hostRules: Int? = null,
    val exclusions: Int? = null,
    val paused: Boolean = false,
    @SerialName("resume_at") val resumeAt: Long? = null,
    val ipv6: Boolean? = null,
    @SerialName("lists_updated") val listsUpdated: Long? = null,
    @SerialName("url_filtering") val urlFiltering: Boolean? = null,
    val scope: String = "chosen",
    // Current daemons report the setting, which defaults to on. Older daemons may omit it.
    val cosmetic: Boolean? = null,
    @SerialName("boot_guard") val bootGuard: Long? = null,
)

fun parseStatus(text: String): DaemonStatus = reading("status") { json.decodeFromString<StatusDto>(text).toStatus() }

private fun StatusDto.toStatus() = DaemonStatus(
    running = running,
    pid = pid,
    startedAt = started.toInstant(),
    version = version,
    apps = apps,
    rules = rules,
    hostRules = hostRules,
    exclusions = exclusions,
    paused = paused,
    resumeAt = resumeAt.toInstant(),
    ipv6 = ipv6,
    listsUpdated = listsUpdated.toInstant(),
    urlFiltering = urlFiltering,
    filteringScope = scope.toFilteringScope(),
    cosmetic = cosmetic,
    bootGuard = bootGuard.toInstant(),
    filteringApps = filteringApps,
)

@Serializable
private class AppSelectionsDto(val scope: String, val chosen: Set<String>, val excluded: Set<String>)

private fun String.toFilteringScope() = if (this == "all") FilteringScope.All else FilteringScope.Chosen

fun parseAppSelections(text: String): AppSelections = reading("app selections") {
    val selections = json.decodeFromString<AppSelectionsDto>(text)
    AppSelections(selections.scope.toFilteringScope(), selections.chosen, selections.excluded)
}

@Serializable
private class AppDto(
    @SerialName("package") val packageName: String,
    @SerialName("profile_serial") val profileSerial: Long? = null,
    @SerialName("paused_until") val pausedUntil: Long? = null,
    val effective: EffectiveDto,
    @SerialName("unavailable_reason") val unavailableReason: String? = null,
)

@Serializable
private class EffectiveDto(val http: LevelDto, val https: LevelDto)

@Serializable
private class LevelDto(val level: String, val reason: String? = null)

private fun String.toFilterLevel(): FilterLevel = if (this == "full") FilterLevel.Full else FilterLevel.Hosts

fun parseApps(text: String): List<FilteredApp> = reading("apps") {
    json.decodeFromString<List<AppDto>>(text).map {
        val http = it.effective.http
        val https = it.effective.https
        val unavailable = it.unavailableReason?.let(::unavailableReason)
            ?: AppUnavailableReason.Unavailable.takeIf { http.level == "off" || https.level == "off" }
        val level = https.level.toFilterLevel().takeIf { unavailable == null }
        val identity = AppIdentity(it.packageName, it.profileSerial)
        FilteredApp(
            packageName = identity.packageName,
            profileSerial = identity.profileSerial,
            pausedUntil = it.pausedUntil.toInstant(),
            httpLevel = http.level.toFilterLevel().takeIf { unavailable == null },
            level = level,
            unavailableReason = unavailable,
            levelReason = when {
                level == null || level == FilterLevel.Full -> null
                https.reason == "url_filtering_off" -> LevelReason.UrlFilteringOff
                https.reason == "ca_not_installed" -> LevelReason.CaNotInstalled
                https.reason == "exclusions_pending" -> LevelReason.ExclusionsPending
                https.reason == "profile_trust_unavailable" -> LevelReason.ProfileTrustUnavailable
                else -> LevelReason.Unknown
            },
        )
    }
}

private fun unavailableReason(value: String): AppUnavailableReason = when (value) {
    "shared_uid" -> AppUnavailableReason.SharedUid
    else -> AppUnavailableReason.Unavailable
}

@Serializable
private class AppEligibilityDto(val apps: Map<String, String>, val groups: List<SharedUidGroup> = emptyList())

data class AppEligibility(val apps: Map<String, AppUnavailableReason?>, val groups: List<SharedUidGroup> = emptyList())

@Serializable
data class SharedUidGroup(val id: String, val packages: Set<String>) {
    init {
        require(id.isNotEmpty() && ':' !in id && '@' !in id && packages.isNotEmpty())
    }

    val target: String get() = "@$id"
}

internal fun parseAppEligibility(text: String): AppEligibility = reading("app eligibility") {
    val dto = json.decodeFromString<AppEligibilityDto>(text)
    AppEligibility(dto.apps.mapValues { (_, value) -> if (value == "eligible") null else unavailableReason(value) }, dto.groups)
}

@Serializable
data class WorkProfile(
    val serial: Long,
    /** Null if this known profile could not be inventoried. */
    val packages: Set<String>?,
    val groups: List<SharedUidGroup>? = emptyList(),
)

@Serializable
private class ProfilesDto(val profiles: List<WorkProfile>)

internal fun parseWorkProfiles(text: String): List<WorkProfile> = reading("app profiles") {
    json.decodeFromString<ProfilesDto>(text).profiles.also { profiles ->
        require(profiles.all { it.serial > 0 } && profiles.distinctBy { it.serial }.size == profiles.size)
    }
}

/** How many days `abd stats --json` returns. */
internal const val STATS_DAYS = 7

@Serializable
private class StatsDto(val days: List<JsonArray>, val apps: Map<String, List<JsonArray>>)

fun parseStats(text: String): DailyStats = reading("stats") { json.decodeFromString<StatsDto>(text).toDailyStats() }

private fun StatsDto.toDailyStats() = DailyStats(
    days = days.toBlockedDays(),
    apps = apps.mapValues { (_, days) -> days.toBlockedDays() },
)

private fun List<JsonArray>.toBlockedDays(): List<BlockedCount> {
    require(size == STATS_DAYS)
    return map { pair ->
        require(pair.size == 2)
        BlockedCount(requests = pair[0].blockedCount(), hosts = pair[1].blockedCount())
    }
}

private fun JsonElement.blockedCount(): Long {
    val value = jsonPrimitive
    val count = value.longOrNull
    require(!value.isString && count != null && count >= 0)
    return count
}

@Serializable
private class ListDto(
    val url: String,
    val enabled: Boolean,
    val title: String? = null,
    val rules: Int? = null,
    val updated: Long? = null,
    val error: String? = null,
    @SerialName("using_previous_address") val usingPreviousAddress: Boolean = false,
)

fun parseLists(text: String): List<FilterList> = reading("lists") {
    json.decodeFromString<List<ListDto>>(text).map(ListDto::toFilterList)
}

private fun ListDto.toFilterList() = FilterList(url, enabled, title, rules, updated.toInstant(), error, usingPreviousAddress)

@Serializable
private class OverviewDto(val status: StatusDto, val stats: StatsDto, val lists: List<ListDto>)

internal fun parseOverview(text: String): AbdOverview = reading("overview") {
    val dto = json.decodeFromString<OverviewDto>(text)
    AbdOverview(dto.status.toStatus(), dto.stats.toDailyStats(), dto.lists.map(ListDto::toFilterList))
}

@Serializable
private class UpdateDto(
    val event: String,
    val lists: Int = 0,
    val url: String? = null,
    val state: String? = null,
    val rules: Int? = null,
    val error: String? = null,
    val updated: Int = 0,
    val unchanged: Int = 0,
    val failed: Int = 0,
    @SerialName("exclusions_ready") val exclusionsReady: JsonElement? = null,
    @SerialName("exclusions_failed") val exclusionsFailed: JsonElement? = null,
)

/** One line of `abd update --json`, or null for an event this app does not know. */
fun parseUpdateEvent(line: String): UpdateEvent? = reading("update") {
    val dto = json.decodeFromString<UpdateDto>(line)
    when (dto.event) {
        "start" -> UpdateEvent.Started(dto.lists)

        "done" -> {
            val ready = (dto.exclusionsReady as? JsonPrimitive)?.takeUnless { it.isString }
            val failed = (dto.exclusionsFailed as? JsonPrimitive)?.takeUnless { it.isString }
            UpdateEvent.Finished(
                dto.updated,
                dto.unchanged,
                dto.failed,
                ready?.content?.toBooleanStrictOrNull()
                    ?: throw AbdException.BadOutput("update: invalid exclusions readiness"),
                failed?.intOrNull?.takeIf { it >= 0 && it.toString() == failed.content }
                    ?: throw AbdException.BadOutput("update: invalid exclusions failures"),
            )
        }

        "list" -> {
            val state = when (dto.state) {
                "fetching" -> ListUpdateState.Fetching
                "updated" -> ListUpdateState.Updated
                "unchanged" -> ListUpdateState.Unchanged
                "failed" -> ListUpdateState.Failed
                else -> null
            }
            if (dto.url == null || state == null) null else UpdateEvent.ListProgress(dto.url, state, dto.rules, dto.error)
        }

        else -> null
    }
}

@Serializable
private class PinnedDto(val app: String, val host: String, val since: Long)

fun parsePinned(text: String): List<PinnedHost> = reading("pinned") {
    json.decodeFromString<List<PinnedDto>>(text).map {
        val identity = AppIdentity.fromKey(it.app)
        PinnedHost(identity.packageName, it.host, Instant.ofEpochSecond(it.since), identity.profileSerial)
    }
}

fun parseAllowList(text: String): List<String> = reading("allow") { json.decodeFromString<List<String>>(text) }

@Serializable
private class IssueDto(val line: Int, val text: String, val kind: String, val reason: String? = null)

@Serializable
private class RulesCheckDto(val rules: Int, val issues: List<IssueDto>)

fun parseRulesCheck(text: String): RulesCheck = reading("rules check") {
    val dto = json.decodeFromString<RulesCheckDto>(text)
    RulesCheck(
        rules = dto.rules,
        issues = dto.issues.map {
            val kind = when (it.kind) {
                "unsupported", "cosmetic", "unsupported_include" -> RuleIssueKind.Unsupported
                "inapplicable" -> RuleIssueKind.Inapplicable
                "untrusted" -> RuleIssueKind.Untrusted
                else -> RuleIssueKind.Invalid
            }
            RuleIssue(it.line, it.text, kind, it.reason)
        },
    )
}

@Serializable
private class CheckDto(val blocked: Boolean, val rule: String? = null, val source: String? = null)

fun parseCheck(text: String): RuleCheckResult = reading("check") {
    val dto = json.decodeFromString<CheckDto>(text)
    RuleCheckResult(
        blocked = dto.blocked,
        rule = dto.rule,
        source = when (dto.source) {
            null -> null
            "rules" -> RuleSource.UserRules
            else -> RuleSource.List(dto.source)
        },
    )
}

@Serializable
private class WatchDto(
    val time: Long,
    val app: String,
    val verdict: String,
    val host: String,
    val url: String? = null,
    val page: String? = null,
    val kind: String? = null,
    val reason: String? = null,
)

/**
 * One line of `abd watch --json`, given [id] if it is a request. Null for an
 * event this app does not know.
 */
fun parseWatchLine(line: String, id: Long): WatchEvent? = reading("watch") {
    val obj: JsonObject = json.parseToJsonElement(line).jsonObject
    val dropped = obj["dropped"]?.jsonPrimitive?.longOrNull
    if (dropped != null) return@reading WatchEvent.Dropped(dropped)
    val dto = json.decodeFromJsonElement(WatchDto.serializer(), obj)
    val verdict = when (dto.verdict) {
        "blocked" -> Verdict.Blocked
        "allowed" -> Verdict.Allowed
        "host_blocked" -> Verdict.HostBlocked
        "passed" -> Verdict.Passed
        else -> return@reading null
    }
    val identity = AppIdentity.fromKey(dto.app)
    WatchEvent.Request(
        RequestEvent(
            id = id,
            time = Instant.ofEpochMilli(dto.time),
            packageName = identity.packageName,
            profileSerial = identity.profileSerial,
            verdict = verdict,
            host = dto.host,
            url = dto.url,
            page = dto.page,
            kind = dto.kind,
            reason = dto.reason?.let(::passReason),
        ),
    )
}

private fun passReason(name: String): PassReason = when (name) {
    "pinned" -> PassReason.Pinned
    "hosts_only" -> PassReason.DomainOnly
    "exclusions_pending" -> PassReason.ExclusionsPending
    "excluded" -> PassReason.Excluded
    "trusted" -> PassReason.Trusted
    "sensitive" -> PassReason.Sensitive
    "no_name" -> PassReason.NoName
    "untrusted" -> PassReason.Untrusted
    "unverified" -> PassReason.Unverified
    else -> PassReason.Other
}
