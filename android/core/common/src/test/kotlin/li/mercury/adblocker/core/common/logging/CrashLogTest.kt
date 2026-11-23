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

package li.mercury.adblocker.core.common.logging

import java.io.IOException
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertFalse

class CrashLogTest {
    @Test
    fun aCrashIsLoggedWithItsClassesAndFramesButNotItsMessages() {
        val cause = IOException("cannot reach ads.example.com")
        val crash = IllegalStateException("com.example.bank failed", cause)

        val logged = crash.withoutMessages()

        assertContains(logged, "java.lang.IllegalStateException")
        assertContains(logged, "Caused by: java.io.IOException")
        assertContains(logged, "\tat li.mercury.adblocker.core.common.logging.CrashLogTest")
        assertFalse("ads.example.com" in logged)
        assertFalse("com.example.bank" in logged)
    }

    @Test
    fun aCauseThatLoopsBackIsWrittenOnce() {
        val first = RuntimeException("a")
        val second = RuntimeException("b", first)
        first.initCause(second)

        val logged = first.withoutMessages()

        assertContains(logged, "Caused by: java.lang.RuntimeException")
    }
}
