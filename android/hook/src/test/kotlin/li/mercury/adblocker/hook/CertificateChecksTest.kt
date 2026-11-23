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

import java.net.Socket
import java.security.cert.Certificate
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedTrustManager
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse

class CertificateChecksTest {
    /** The methods of OkHttp 4 and 5's pinner, as `javap` shows them. */
    @Suppress("UNUSED_PARAMETER", "FunctionName")
    private class Pinner {
        fun check(hostname: String, peerCertificates: List<Certificate>) = Unit

        fun check(hostname: String, vararg peerCertificates: Certificate) = Unit

        fun `check$okhttp`(hostname: String, cleaned: () -> List<X509Certificate>) = Unit

        fun findMatchingPins(hostname: String): List<Any> = emptyList()
    }

    /** A trust manager with every form of the server check Android calls. */
    @Suppress("UNUSED_PARAMETER")
    private class Manager : X509ExtendedTrustManager() {
        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) = Unit

        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String, socket: Socket) = Unit

        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String, engine: SSLEngine) = Unit

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) = Unit

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String, socket: Socket) = Unit

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String, engine: SSLEngine) = Unit

        fun checkServerTrusted(chain: Array<X509Certificate>, authType: String, host: String): List<X509Certificate> = chain.toList()

        fun checkServerTrusted(host: String): Boolean = true

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    @Test
    fun givenOkHttpsPinner_whenMatched_thenTheListAndFunctionChecksAreHooked() {
        val hooked = Pinner::class.java.declaredMethods.filter(::isOkHttpCheck)

        assertEquals(
            setOf("check(List)", "check\$okhttp(Function0)"),
            hooked.map { "${it.name}(${it.parameterTypes[1].simpleName})" }.toSet(),
        )
    }

    @Test
    fun givenATrustManager_whenMatched_thenEveryServerCheckIsHookedAndNothingElse() {
        val hooked = Manager::class.java.declaredMethods.filter(::isServerCheck)

        assertEquals(4, hooked.size)
        assertEquals(setOf("checkServerTrusted"), hooked.map { it.name }.toSet())
        assertFalse(X509ExtendedTrustManager::class.java.declaredMethods.any(::isServerCheck), "abstract")
    }

    @Test
    fun givenAndroidsOwnClasses_whenAsked_thenTheyAreNotTheApps() {
        assertFalse(isAppClass(String::class.java))
        assertFalse(isAppClass(X509ExtendedTrustManager::class.java))
    }
}
