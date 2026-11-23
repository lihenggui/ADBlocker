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

import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLPeerUnverifiedException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertSame

/**
 * A pin check of an app, as OkHttp's pinner fails, met by the policy the
 * hooks apply.
 */
class RelaxationTest {
    @get:Rule
    val folder = TemporaryFolder()

    private val authority = TestAuthority()
    private val public = PublicAuthority()
    private val stores by lazy { Stores(folder.newFolder("system"), folder.newFolder("user")) }
    private val platform = PkixTrust(authority.system.certificate, public.root.certificate)

    private fun policy(mounted: Boolean = true): PinPolicy {
        if (mounted) authority.mountInto(stores)
        return PinPolicy(LocalAuthority({ stores }, { moduleMountInfo(stores) }), platform)
    }

    /** What the hook for OkHttp's pinner does with a check that throws [failure]. */
    private fun pinCheck(policy: PinPolicy, presented: List<X509Certificate>, failure: Throwable?): Any? = relaxed(
        check = { if (failure != null) throw failure else "passed" },
        isCheckFailure = { it is SSLPeerUnverifiedException },
        accept = { policy.validatedToLocalRoot(presented, null, "a.example")?.let { Accepted(null) } },
    )

    @Test
    fun givenAChainEndingInTheLocalRoot_whenThePinCheckFails_thenItPasses() {
        val failure = SSLPeerUnverifiedException("Certificate pinning failure!")

        assertNull(pinCheck(policy(), authority.presented("a.example"), failure))
    }

    @Test
    fun givenAChainEndingInAnotherRoot_whenThePinCheckFails_thenItsFailureStands() {
        val failure = SSLPeerUnverifiedException("Certificate pinning failure!")

        val thrown = assertFailsWith<SSLPeerUnverifiedException> {
            pinCheck(policy(), public.presented("a.example"), failure)
        }
        assertSame(failure, thrown)
    }

    @Test
    fun givenAProcessWithoutTheLocalRoot_whenThePinCheckFails_thenItsFailureStands() {
        val failure = SSLPeerUnverifiedException("Certificate pinning failure!")

        val thrown = assertFailsWith<SSLPeerUnverifiedException> {
            pinCheck(policy(mounted = false), authority.presented("a.example"), failure)
        }
        assertSame(failure, thrown)
        assertEquals(0, platform.calls)
    }

    @Test
    fun givenAPinCheckThatPasses_whenRun_thenItsResultIsReturnedAndNothingIsDecided() {
        assertEquals("passed", pinCheck(policy(), authority.presented("a.example"), failure = null))
        assertEquals(0, platform.calls)
    }

    @Test
    fun givenAnotherFailureOfTheCheck_whenRun_thenItIsThrownWithoutDeciding() {
        val failure = IllegalStateException("closed")

        val thrown = assertFailsWith<IllegalStateException> {
            pinCheck(policy(), authority.presented("a.example"), failure)
        }
        assertSame(failure, thrown)
        assertEquals(0, platform.calls)
    }

    @Test
    fun givenADecisionThatThrows_whenThePinCheckFails_thenItsFailureStands() {
        val failure = CertificateException("Pin verification failed")

        val thrown = assertFailsWith<CertificateException> {
            relaxed(
                check = { throw failure },
                isCheckFailure = { it is CertificateException },
                accept = { error("the hook broke") },
            )
        }
        assertSame(failure, thrown)
    }

    @Test
    fun givenAcceptedWithAResult_whenTheCheckFails_thenTheResultIsReturned() {
        val chain = authority.presented("a.example")

        val result = relaxed(
            check = { throw CertificateException("Pin verification failed") },
            isCheckFailure = { it is CertificateException },
            accept = { Accepted(chain) },
        )

        assertSame(chain, result)
    }

    @Test
    fun givenAFunctionThatReturnsTheChain_whenItsCertificatesAreRead_thenTheChainIsReturned() {
        val chain = authority.presented("a.example")
        val cleaned: () -> List<X509Certificate> = { chain }

        assertSame(chain, certificatesOf(cleaned, Function0::class.java))
        assertSame(chain, certificatesOf(chain, List::class.java))
        assertNull(certificatesOf(cleaned, Any::class.java))
        assertNull(certificatesOf(null, Function0::class.java))
    }
}
