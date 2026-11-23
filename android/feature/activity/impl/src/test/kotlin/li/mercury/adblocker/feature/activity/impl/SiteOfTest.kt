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
}
