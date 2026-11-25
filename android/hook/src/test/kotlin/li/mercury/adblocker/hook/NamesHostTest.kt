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

package li.mercury.adblocker.hook

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class NamesHostTest {
    private val issuer = selfSigned("Example Root", organization = "Example")

    @Test
    fun givenALeafForAName_whenAskedForThatName_thenItIsNamedWhateverItsCaseOrFinalDot() {
        val leaf = leaf("a.example", issuer)

        assertTrue(namesHost(leaf, "a.example"))
        assertTrue(namesHost(leaf, "A.EXAMPLE."))
        assertFalse(namesHost(leaf, "b.example"))
        assertFalse(namesHost(leaf, "x.a.example"))
    }

    @Test
    fun givenAWildcardLeaf_whenAsked_thenItNamesOneLabelOnly() {
        val leaf = leaf("*.a.example", issuer)

        assertTrue(namesHost(leaf, "x.a.example"))
        assertFalse(namesHost(leaf, "a.example"))
        assertFalse(namesHost(leaf, "y.x.a.example"))
        assertFalse(namesHost(leaf, ".a.example"))
    }

    @Test
    fun givenALeafForAnAddress_whenAskedForIt_thenOnlyThatAddressIsNamed() {
        val leaf = leaf("192.0.2.1", issuer)

        assertTrue(namesHost(leaf, "192.0.2.1"))
        assertFalse(namesHost(leaf, "192.0.2.2"))
    }

    @Test
    fun givenAnIpv6San_whenComparedWithOtherSpellings_thenOnlyTheSameNumericAddressMatches() {
        val leaf = leaf("2001:db8:0:0:0:0:0:1", issuer)

        assertTrue(namesHost(leaf, "2001:db8::1"))
        assertTrue(namesHost(leaf, "[2001:0DB8:0:0:0:0:0:0001]"))
        assertFalse(namesHost(leaf, "2001:db8::2"))
        assertFalse(namesHost(leaf, "2001:db8:::1"))
        assertFalse(namesHost(leaf, "a.example"))
    }

    @Test
    fun givenAnIpv6San_whenItsLastGroupsAreWrittenAsIpv4_thenTheAddressStillMatches() {
        val leaf = leaf("2001:db8::c000:201", issuer)

        assertTrue(namesHost(leaf, "2001:db8::192.0.2.1"))
        assertFalse(namesHost(leaf, "192.0.2.1"))
        assertFalse(namesHost(leaf, "2001:db8::192.0.2.257"))
    }

    @Test
    fun givenARootWithoutAlternativeNames_whenAskedForItsCommonName_thenItIsNotNamed() {
        assertFalse(namesHost(issuer.certificate, "Example Root"))
    }
}
