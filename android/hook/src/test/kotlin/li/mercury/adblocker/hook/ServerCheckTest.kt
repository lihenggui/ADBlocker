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
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame

class ServerCheckTest {
    private val chain = TestAuthority().presented("a.example").toTypedArray()

    private fun types(vararg rest: Class<*>): Array<Class<*>> = arrayOf(Array<X509Certificate>::class.java, *rest)

    @Test
    fun givenTheHostAwareFormWithoutAnAuthType_whenRead_thenTheHostIsNotTakenForIt() {
        val check = assertNotNull(
            ServerCheck.of(types(String::class.java, String::class.java), listOf(chain, null, "a.example")),
        )

        assertNull(check.authType)
        assertEquals("a.example", check.host)
        assertEquals(chain.asList(), check.chain)
    }

    @Test
    fun givenTheFormWithOcspAndSctData_whenRead_thenTheAuthTypeAndHostComeAfterThem() {
        val parameters = types(ByteArray::class.java, ByteArray::class.java, String::class.java, String::class.java)

        val check = assertNotNull(ServerCheck.of(parameters, listOf(chain, null, null, "GENERIC", "a.example")))

        assertEquals("GENERIC", check.authType)
        assertEquals("a.example", check.host)
    }

    @Test
    fun givenTheShortFormOrASocketThatIsNotTls_whenRead_thenThereIsNoHost() {
        assertNull(assertNotNull(ServerCheck.of(types(String::class.java), listOf(chain, "RSA"))).host)
        val check = ServerCheck.of(types(String::class.java, Socket::class.java), listOf(chain, "RSA", Socket()))
        assertEquals("RSA", assertNotNull(check).authType)
        assertNull(check.host)
    }

    @Test
    fun givenTheEngineForm_whenRead_thenTheEngineIsKeptForThePlatformsOwnCheck() {
        val engine = identifyingEngine("a.example")

        val check = ServerCheck.of(types(String::class.java, SSLEngine::class.java), listOf(chain, "RSA", engine))

        assertSame(engine, assertNotNull(check).connection)
        assertEquals("a.example", check.host)
    }

    @Test
    fun givenAnotherForm_whenRead_thenItIsNotACheckToRelax() {
        assertNull(ServerCheck.of(types(String::class.java, Int::class.java), listOf(chain, "RSA", 1)))
        assertNull(ServerCheck.of(arrayOf(String::class.java), listOf("a.example")))
        assertNull(ServerCheck.of(types(String::class.java), listOf(chain)))
    }
}
