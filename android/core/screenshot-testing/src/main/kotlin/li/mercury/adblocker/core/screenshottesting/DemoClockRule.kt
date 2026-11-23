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

package li.mercury.adblocker.core.screenshottesting

import android.os.SystemClock
import org.junit.rules.ExternalResource
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import java.util.TimeZone

internal val demoClock = Clock.fixed(Instant.parse("2026-11-19T15:00:00Z"), ZoneOffset.UTC)

/** Aligns the framework clock and the zone used for local timestamps before Compose starts. */
class DemoClockRule : ExternalResource() {
    private var previous: TimeZone? = null

    val now: Instant get() = demoClock.instant()

    override fun before() {
        previous = TimeZone.getDefault()
        TimeZone.setDefault(TimeZone.getTimeZone(demoClock.zone))
        // With a paused looper, Robolectric's wall clock starts at its
        // uptime, in 1970; it can only move forward.
        SystemClock.setCurrentTimeMillis(now.toEpochMilli())
    }

    override fun after() {
        TimeZone.setDefault(previous)
    }
}
