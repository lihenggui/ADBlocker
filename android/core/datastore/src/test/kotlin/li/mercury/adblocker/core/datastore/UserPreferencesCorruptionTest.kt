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

package li.mercury.adblocker.core.datastore

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import li.mercury.adblocker.core.datastore.di.userPreferencesStore
import li.mercury.adblocker.core.model.UserData
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import kotlin.test.Test
import kotlin.test.assertEquals

class UserPreferencesCorruptionTest {
    @get:Rule
    val temporary = TemporaryFolder()

    @Test
    fun givenAnUnreadableFile_whenTheSettingsAreRead_thenTheyAreTheDefaultsAndLaterWritesWork() = runBlocking {
        val file = temporary.newFile("user_preferences.preferences_pb")
        file.writeBytes(ByteArray(64) { 0xFF.toByte() })
        val scope = CoroutineScope(Dispatchers.IO)
        try {
            val settings = UserPreferencesDataSource(userPreferencesStore(scope) { file })

            assertEquals(UserData(), settings.userData.first())
            settings.setOnboardingDone(true)
            assertEquals(true, settings.userData.first().onboardingDone)
        } finally {
            scope.cancel()
        }
    }
}
