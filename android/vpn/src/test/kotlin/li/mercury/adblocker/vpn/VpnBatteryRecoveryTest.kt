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

package li.mercury.adblocker.vpn

import li.mercury.adblocker.core.model.VpnState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.test.fail

class VpnBatteryRecoveryTest {
    @Test
    fun givenAnObservedInterruption_whenStoppedWithConsentAndRestricted_thenOfferExemption() {
        assertTrue(shouldOfferBatteryExemption(VpnState.Off, prepared = true, clear = { fail("Cleared recovery state") }) { true })
    }

    @Test
    fun givenNoInterruption_whenNeverStartedStoppedOrDismissed_thenSkipExemption() {
        assertFalse(shouldOfferBatteryExemption(VpnState.Off, prepared = true, clear = { fail("Cleared recovery state") }) { false })
    }

    @Test
    fun givenTheVpnCannotOfferRecovery_whenChecked_thenSkipTheInterruptionLookup() {
        for (state in VpnState.entries - VpnState.Off) {
            var cleared = false
            val offered = shouldOfferBatteryExemption(state, prepared = true, clear = { cleared = true }) {
                fail("Read interruption state")
            }
            assertFalse(offered)
            assertEquals(state == VpnState.Revoked || state == VpnState.Failed, cleared)
        }

        var cleared = false
        val offered = shouldOfferBatteryExemption(VpnState.Off, prepared = false, clear = { cleared = true }) {
            fail("Read interruption state after consent was lost")
        }
        assertFalse(offered)
        assertTrue(cleared)
    }
}
