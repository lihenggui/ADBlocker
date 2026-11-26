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

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** Trusting a site from a request covers the site, not only the host that was asked. */
class SiteOfTest {
    @Test
    fun givenASubdomain_whenTrusted_thenTheSiteIsItsLastTwoLabels() {
        assertEquals("adnet.com", siteOf("ads.cdn.adnet.com"))
    }

    @Test
    fun givenACountryCodeSecondLevel_whenTrusted_thenThreeLabelsAreKept() {
        assertEquals("news.co.uk", siteOf("img.news.co.uk"))
    }

    @Test
    fun givenASiteAlready_whenTrusted_thenItIsKept() {
        assertEquals("bank.example", siteOf("bank.example"))
        assertEquals("bank.com", siteOf("WWW.bank.COM."))
    }

    @Test
    fun aPrivateSuffixDoesNotTrustOtherTenantsOrTheProvider() {
        assertEquals("alice.github.io", siteOf("img.alice.github.io"))
        assertEquals("alice.blogspot.com", siteOf("alice.blogspot.com"))
        assertNull(siteOf("github.io"))
        assertNull(siteOf("co.uk"))
        assertEquals("city.kawasaki.jp", siteOf("cdn.city.kawasaki.jp"))
        assertNull(siteOf("other.kawasaki.jp"))
    }

    @Test
    fun anUnknownSuffixOrAnAddressIsNeverShortened() {
        assertEquals("img.shop.example", siteOf("img.shop.example"))
        assertEquals("192.0.2.12", siteOf("192.0.2.12"))
        assertEquals("2001:db8::1", siteOf("2001:db8::1"))
        assertNull(siteOf("localhost"))
        assertNull(siteOf("invalid..com"))
    }

    @Test
    fun givenSuffixRules_whenFindingASite_thenWildcardExceptionsAndInternationalLabelsAreRespected() {
        assertNull(siteOf("subdomain.ck"))
        assertEquals("site.subdomain.ck", siteOf("www.site.subdomain.ck"))
        assertEquals("www.ck", siteOf("cdn.www.ck"))
        assertNull(siteOf("xn--55qx5d.cn"))
        assertEquals("site.xn--55qx5d.cn", siteOf("www.site.xn--55qx5d.cn"))
    }

    @Test
    fun givenNumericAddresses_whenFindingASite_thenTheEntireAddressIsKept() {
        for (host in listOf("0.0.0.0", "::", "::ffff:192.0.2.1", "fe80::1%eth0", "1:2:3:4:5:6:7:8")) {
            assertEquals(host, siteOf(host), host)
        }
        assertEquals("2001:db8::1", siteOf("2001:DB8::1"))
    }

    @Test
    fun givenMalformedNamesAndAddresses_whenFindingASite_thenTheyAreRejected() {
        for (host in listOf(
            "192.0.2.01", "256.0.0.1", "0x7f.0.0.1", "::ffff:192.0.2.1%eth0",
            ":1:2:3:4:5:6:7", "1:2:3:4:5:6:7:8::", "2001:::1", "2001::db8::1", "[::1]",
            "www.-bad.com", "www.bad-.com", "www.numeric.1a", "${"x".repeat(64)}.com",
        )) {
            assertNull(siteOf(host), host)
        }
    }
}
