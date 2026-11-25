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

import android.content.Context
import android.content.res.Configuration
import androidx.test.core.app.ApplicationProvider
import li.mercury.adblocker.core.model.RuleIssue
import li.mercury.adblocker.core.model.RuleIssueCode
import li.mercury.adblocker.core.model.RuleIssueCode.Known
import li.mercury.adblocker.core.model.RuleIssueKind
import li.mercury.adblocker.core.model.RuleIssueReason
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.util.Locale
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
class RuleIssueTextTest {
    private fun message(kind: RuleIssueKind, reason: RuleIssueReason?) = RuleIssue(1, "", kind, reason).message()

    @Test
    fun givenRuleReasons_whenMapped_thenKnownDetailsAndUnknownCodesUseLocalizedMessages() {
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_invalid_option, "typo"), message(RuleIssueKind.Invalid, RuleIssueReason(Known.Option, "typo")))
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_unsupported_named, "header"), message(RuleIssueKind.Unsupported, RuleIssueReason(Known.Option, "header")))
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_inapplicable_named, "popup"), message(RuleIssueKind.Inapplicable, RuleIssueReason(Known.Option, "popup")))
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_invalid), message(RuleIssueKind.Invalid, RuleIssueReason(RuleIssueCode.Other("future"), "detail")))
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_unsupported_include), message(RuleIssueKind.Unsupported, RuleIssueReason(Known.IncludeNotFollowed)))
        assertEquals(RuleIssueMessage(R.string.feature_filters_issue_malformed_directive), message(RuleIssueKind.Invalid, RuleIssueReason(Known.MalformedDirective)))
    }

    @Test
    fun givenBothLocales_whenFormattingIssues_thenEveryClassHasTextAndNamesAreVisible() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val defaults = RuleIssueKind.entries.map { message(it, null) }
        val named = listOf(
            message(RuleIssueKind.Invalid, RuleIssueReason(Known.Option, "typo")),
            message(RuleIssueKind.Unsupported, RuleIssueReason(Known.ScriptletNoCounterpart, "example-scriptlet")),
            message(RuleIssueKind.Inapplicable, RuleIssueReason(Known.Option, "popup")),
        )
        val texts = listOf(Locale.ENGLISH, Locale.SIMPLIFIED_CHINESE).map { locale ->
            val config = Configuration(context.resources.configuration).apply { setLocale(locale) }
            val localized = context.createConfigurationContext(config)
            defaults.map { localized.getString(it.resource) } + named.map {
                localized.getString(it.resource, it.name).also { text -> assertTrue(text.contains(it.name!!)) }
            }
        }
        texts[0].zip(texts[1]).forEach { (english, chinese) ->
            assertTrue(english.isNotBlank() && chinese.isNotBlank() && english != chinese)
        }
    }
}
