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
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class PinPolicyTest {
    @get:Rule
    val folder = TemporaryFolder()

    private val authority = TestAuthority()
    private val public = PublicAuthority()
    private val stores by lazy { Stores(folder.newFolder("system"), folder.newFolder("user")) }

    private fun policy(platform: PlatformTrust, mounted: Boolean = true): PinPolicy {
        if (mounted) authority.mountInto(stores)
        return PinPolicy(LocalAuthority({ stores }, { moduleMountInfo(stores) }), platform)
    }

    @Test
    fun givenAChainValidatedToTheSystemRoot_whenChecked_thenItEndsInALocalRoot() {
        val validated = PkixTrust(authority.system.certificate).validate(authority.presented("a.example"), "RSA", null, null)

        assertTrue(policy(PkixTrust()).endsInLocalRoot(validated))
    }

    @Test
    fun givenAChainValidatedToTheUserRoot_whenChecked_thenItEndsInALocalRoot() {
        // As in an app that trusts the user's certificates too.
        val validated = PkixTrust(authority.user.certificate).validate(authority.presented("a.example"), "RSA", null, null)
        assertEquals(authority.user.certificate, validated.last())

        assertTrue(policy(PkixTrust()).endsInLocalRoot(validated))
    }

    @Test
    fun givenAChainValidatedToAnotherRoot_whenChecked_thenItDoesNotEndInALocalRoot() {
        val validated = PkixTrust(public.root.certificate).validate(public.presented("a.example"), "RSA", null, null)

        assertFalse(policy(PkixTrust()).endsInLocalRoot(validated))
    }

    @Test
    fun givenARootOfTheSameNameAndAnotherKey_whenChecked_thenItDoesNotEndInALocalRoot() {
        val impostor = selfSigned("ADBlocker system CA")
        val chain = listOf(leaf("a.example", impostor), impostor.certificate)

        assertFalse(policy(PkixTrust()).endsInLocalRoot(chain))
    }

    @Test
    fun givenAProcessWithoutTheLocalRoots_whenChecked_thenNothingEndsInThemAndNothingIsValidated() {
        val platform = PkixTrust(authority.system.certificate)
        val policy = policy(platform, mounted = false)
        val validated = PkixTrust(authority.system.certificate).validate(authority.presented("a.example"), "RSA", null, null)

        assertFalse(policy.endsInLocalRoot(validated))
        assertNull(policy.validatedToLocalRoot(authority.presented("a.example"), "RSA", "a.example"))
        assertEquals(0, platform.calls)
    }

    @Test
    fun givenAPresentedChainOfTheLocalAuthority_whenValidated_thenThePlatformsChainIsReturned() {
        val presented = authority.presented("a.example")

        val validated = policy(PkixTrust(authority.system.certificate)).validatedToLocalRoot(presented, null, "a.example")

        assertEquals(presented + authority.system.certificate, assertNotNull(validated))
    }

    @Test
    fun givenAPresentedChainOfAnotherAuthority_whenValidated_thenThePlatformIsNotAsked() {
        val platform = PkixTrust(public.root.certificate)

        assertNull(policy(platform).validatedToLocalRoot(public.presented("a.example"), "RSA", "a.example"))
        assertEquals(0, platform.calls)
    }

    @Test
    fun givenAChainThePlatformDoesNotTrust_whenValidated_thenNothingIsReturned() {
        // An app whose configuration trusts other roots than the module's.
        val platform = PkixTrust(public.root.certificate)

        assertNull(policy(platform).validatedToLocalRoot(authority.presented("a.example"), "RSA", "a.example"))
        assertEquals(1, platform.calls)
    }

    @Test
    fun givenAnotherRootOfTheSameNameThatThePlatformTrusts_whenValidated_thenNothingIsReturned() {
        // Say the user installed a certificate named as the module's.
        val impostor = selfSigned("ADBlocker user CA")
        val presented = listOf(leaf("a.example", impostor))

        assertNull(policy(PkixTrust(impostor.certificate)).validatedToLocalRoot(presented, "RSA", "a.example"))
    }

    @Test
    fun givenSomethingOtherThanCertificates_whenValidated_thenNothingIsReturned() {
        val presented = listOf<Any>("not a certificate")

        assertNull(policy(PkixTrust(authority.system.certificate)).validatedToLocalRoot(presented, "RSA", null))
        assertFalse(policy(PkixTrust()).endsInLocalRoot(presented))
        assertFalse(policy(PkixTrust()).endsInLocalRoot(emptyList<Any>()))
    }

    @Test
    fun givenEndpointIdentificationOnAnEngineForAnotherHost_whenValidated_thenNothingIsReturned() {
        // The trust manager failed on the server's name, not on a pin.
        val engine = identifyingEngine("b.example")
        val platform = PkixTrust(authority.system.certificate)

        val validated = policy(platform).validatedToLocalRoot(
            authority.presented("a.example"),
            "RSA",
            peerHost(engine),
            engine,
        )

        assertNull(validated)
        assertSame(engine, platform.connection, "the platform checks the engine's form")
    }

    @Test
    fun givenEndpointIdentificationOnAnEngineForTheLeafsHost_whenValidated_thenTheChainIsReturned() {
        val engine = identifyingEngine("a.example")
        val presented = authority.presented("a.example")

        val validated = policy(PkixTrust(authority.system.certificate))
            .validatedToLocalRoot(presented, "RSA", peerHost(engine), engine)

        assertEquals(presented + authority.system.certificate, validated)
    }

    @Test
    fun givenAHostTheLeafDoesNotName_whenValidated_thenNothingIsReturned() {
        val policy = policy(PkixTrust(authority.system.certificate))

        assertNull(policy.validatedToLocalRoot(authority.presented("a.example"), "RSA", "b.example"))
        assertNotNull(policy.validatedToLocalRoot(authority.presented("a.example"), "RSA", "A.Example."))
    }
}
