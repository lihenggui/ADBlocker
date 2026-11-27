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

package li.mercury.adblocker.feature.activity.impl

import androidx.activity.ComponentActivity
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.repository.ActivityBuffer
import li.mercury.adblocker.core.data.repository.ActivityFeed
import li.mercury.adblocker.core.data.repository.ActivityRepository
import li.mercury.adblocker.core.data.repository.RulesRepository
import li.mercury.adblocker.core.model.RequestEvent
import li.mercury.adblocker.core.model.Verdict
import li.mercury.adblocker.core.testing.TestUserDataRepository
import li.mercury.adblocker.core.testing.unexpected
import li.mercury.adblocker.core.ui.AppSnackbarHost
import li.mercury.adblocker.core.ui.LocalSnackbarHostState
import li.mercury.adblocker.core.ui.MessageEffect
import li.mercury.adblocker.feature.activity.api.ActivityFilter
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import java.time.Instant
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class RequestSnackbarLifetimeTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test
    fun givenRetainedViewModels_whenTheListCompositionIsRecreated_thenItCanUndoTheRequestEdit() = runTest {
        val request = RequestEvent(1, Instant.EPOCH, "com.example.news", Verdict.Allowed, "ads.example", null, null, null, null)
        val activity = object : ActivityRepository by unexpected() {
            override fun feed(packageName: String?, buffer: ActivityBuffer?) = flowOf(ActivityFeed(listOf(request), 0))
        }
        val edits = mutableListOf<Pair<String, String>>()
        val rules = object : RulesRepository by unexpected() {
            override suspend fun addRule(rule: String) = edits.add("add" to rule)
            override suspend fun removeRule(rule: String) = edits.add("remove" to rule)
        }
        val recorder = ActivityRecorder(activity, TestUserDataRepository())
        assertTrue(recorder.requestStart())
        recorder.record()
        val activityMessages = ActivityMessages()
        val list = ActivityViewModel(recorder, ActivityFilter(), unexpected(), unexpected(), activityMessages = activityMessages)
        val detail = RequestViewModel(request.id, recorder, unexpected(), rules, activityMessages)
        val listPresent = mutableStateOf(true)
        compose.setContent {
            if (listPresent.value) {
                val host = remember { SnackbarHostState() }
                CompositionLocalProvider(LocalSnackbarHostState provides host) {
                    MessageEffect(list.messages.pending, list.messages::onMessageShown)
                    AppSnackbarHost()
                }
            }
        }

        compose.runOnIdle { listPresent.value = false }
        compose.runOnIdle { detail.blockHost() }
        compose.runOnIdle {
            assertEquals(listOf("add" to "||ads.example^"), edits)
            listPresent.value = true
        }

        val undo = compose.activity.getString(R.string.feature_activity_undo)
        compose.onNodeWithText(undo).performClick()
        compose.runOnIdle { assertEquals(listOf("add" to "||ads.example^", "remove" to "||ads.example^"), edits) }
    }
}
