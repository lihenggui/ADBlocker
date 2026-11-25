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

package li.mercury.adblocker.core.model

import java.time.Instant

/** A filter list subscription, as `abd lists --json` gives it. */
data class FilterList(
    val url: String,
    val enabled: Boolean,
    /** From the list's `! Title:` line; null until it has been downloaded. */
    val title: String?,
    val rules: Int?,
    val updated: Instant?,
    /** Why the last update of this list failed; null after a success. */
    val error: String?,
    /** The active copy was downloaded from the catalog's previous address. */
    val usingPreviousAddress: Boolean = false,
) {
    /** On but never downloaded: it contributes no rules until a download succeeds. One that is off is not downloaded. */
    val awaitsFirstDownload: Boolean get() = enabled && updated == null && !usingPreviousAddress
}

/** What `abd update --json` reports as it goes. */
sealed interface UpdateEvent {
    data class Started(val lists: Int) : UpdateEvent

    data class ListProgress(
        val url: String,
        val state: ListUpdateState,
        val rules: Int?,
        val error: String?,
    ) : UpdateEvent

    data class Finished(
        val updated: Int,
        val unchanged: Int,
        val failed: Int,
        val exclusionsReady: Boolean = true,
        val exclusionsFailed: Int = 0,
    ) : UpdateEvent {
        val hasFailures: Boolean get() = failed > 0 || exclusionsFailed > 0 || !exclusionsReady
    }
}

enum class ListUpdateState { Fetching, Updated, Unchanged, Failed }

/** The user's own rules, checked: those that will not be applied. */
data class RulesCheck(val rules: Int, val issues: List<RuleIssue>)

sealed interface RuleIssueCode {
    enum class Known : RuleIssueCode {
        Option,
        MalformedDirective,
        IncludeNotFollowed,
        ScriptletNoCounterpart,
        ScriptletTrust,
        ScriptletArguments,
    }

    data class Other(val value: String) : RuleIssueCode
}

data class RuleIssueReason(val code: RuleIssueCode, val name: String? = null)

data class RuleIssue(val line: Int, val text: String, val kind: RuleIssueKind, val reason: RuleIssueReason? = null)

enum class RuleIssueKind { Invalid, Unsupported, Inapplicable, Untrusted }

/** What the lists say about one request (`abd check`). */
data class RuleCheckResult(
    val blocked: Boolean,
    val rule: String?,
    val source: RuleSource?,
)

sealed interface RuleSource {
    data class List(val url: String) : RuleSource

    data object UserRules : RuleSource
}
