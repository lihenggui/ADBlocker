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

package li.mercury.adblocker.feature.filters.impl

import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.model.RuleIssueCode.Known
import li.mercury.adblocker.core.model.RuleIssueKind

internal data class RuleIssueMessage(@StringRes val resource: Int, val name: String? = null)

internal fun RuleIssue.message(): RuleIssueMessage {
    val code = reason?.code
    val name = reason?.name
    return when (kind) {
        RuleIssueKind.Invalid -> when {
            code == Known.MalformedDirective -> RuleIssueMessage(R.string.feature_filters_issue_malformed_directive)
            code == Known.Option && name != null -> RuleIssueMessage(R.string.feature_filters_issue_invalid_option, name)
            else -> RuleIssueMessage(R.string.feature_filters_issue_invalid)
        }

        RuleIssueKind.Unsupported -> when {
            code == Known.IncludeNotFollowed -> RuleIssueMessage(R.string.feature_filters_issue_unsupported_include)

            name != null && code in setOf(Known.Option, Known.ScriptletNoCounterpart, Known.ScriptletTrust, Known.ScriptletArguments) ->
                RuleIssueMessage(R.string.feature_filters_issue_unsupported_named, name)

            else -> RuleIssueMessage(R.string.feature_filters_issue_unsupported)
        }

        RuleIssueKind.Inapplicable -> if (code == Known.Option && name != null) {
            RuleIssueMessage(R.string.feature_filters_issue_inapplicable_named, name)
        } else {
            RuleIssueMessage(R.string.feature_filters_issue_inapplicable)
        }

        // Only user-rule checks have a screen. Keep a fallback for newer daemons.
        RuleIssueKind.Untrusted -> RuleIssueMessage(R.string.feature_filters_issue_unsupported)
    }
}

@Composable
internal fun ruleIssueText(issue: RuleIssue): String {
    val (resource, name) = issue.message()
    return if (name == null) stringResource(resource) else stringResource(resource, name)
}
