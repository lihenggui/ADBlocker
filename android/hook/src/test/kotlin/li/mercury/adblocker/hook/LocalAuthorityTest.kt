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
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class LocalAuthorityTest {
    @get:Rule
    val folder = TemporaryFolder()

    private val authority = TestAuthority()
    private val stores by lazy { Stores(folder.newFolder("system"), folder.newFolder("user")) }

    @Test
    fun givenStoresTheModuleMounted_whenRead_thenBothRootsAreAnchors() {
        authority.mountInto(stores)

        val roots = assertNotNull(LocalAuthority({ stores }, { moduleMountInfo(stores) }).roots())

        assertTrue(roots.isAnchor(authority.system.certificate))
        assertTrue(roots.isAnchor(authority.user.certificate))
        assertFalse(roots.isAnchor(PublicAuthority().root.certificate))
    }

    @Test
    fun givenAProcessWithoutTheSystemRoot_whenRead_thenThereAreNoRoots() {
        // As in an app the module did not mount the stores into, where the
        // user may still have installed a certificate of the user root's name.
        writePem(File(stores.user, LocalAuthority.USER_ROOT_FILE), authority.user.certificate)

        assertNull(LocalAuthority({ stores }, { moduleMountInfo(stores) }).roots())
    }

    @Test
    fun givenTheRootFilesWithoutTheModulesMount_whenRead_thenThereAreNoRoots() {
        // As where another module copies certificates into every app's
        // system store: the file is there, the module's mount is not.
        authority.mountInto(stores)
        val copied = moduleMountInfo(stores, systemCopy = "/adb/modules/certificates/system/etc/security/cacerts")

        assertNull(LocalAuthority({ stores }, { copied }).roots())
        assertNull(LocalAuthority({ stores }, { "" }).roots())
    }

    @Test
    fun givenAUserStoreThatIsNotTheModulesCopy_whenRead_thenOnlyTheSystemRootCounts() {
        authority.mountInto(stores)
        val systemOnly = moduleMountInfo(stores).lines().filterNot { ModuleMounts.USER_COPY in it }.joinToString("\n")

        val roots = assertNotNull(LocalAuthority({ stores }, { systemOnly }).roots())

        assertTrue(roots.isAnchor(authority.system.certificate))
        assertFalse(roots.isAnchor(authority.user.certificate))
    }

    @Test
    fun givenAnotherCertificateUnderTheRootsFileName_whenRead_thenThereAreNoRoots() {
        writePem(File(stores.system, LocalAuthority.SYSTEM_ROOT_FILE), PublicAuthority().root.certificate)

        assertNull(LocalAuthority({ stores }, { moduleMountInfo(stores) }).roots())
    }

    @Test
    fun givenARootWithTheSameNameAndAnotherKey_whenCompared_thenItIsNoAnchor() {
        authority.mountInto(stores)
        val impostor = selfSigned("ADBlocker system CA")

        val roots = assertNotNull(LocalAuthority({ stores }, { moduleMountInfo(stores) }).roots())

        assertFalse(roots.isAnchor(impostor.certificate))
    }

    @Test
    fun givenAUserRootThatIsNotTheModules_whenCompared_thenItIsNoAnchor() {
        authority.mountInto(stores)
        writePem(File(stores.user, LocalAuthority.USER_ROOT_FILE), selfSigned("ADBlocker user CA").certificate)

        val roots = assertNotNull(LocalAuthority({ stores }, { moduleMountInfo(stores) }).roots())

        assertFalse(roots.isAnchor(authority.user.certificate))
        assertTrue(roots.isAnchor(authority.system.certificate))
    }

    @Test
    fun givenRootsReadWithinAMinute_whenAskedAgain_thenTheStoresAreNotRead() {
        authority.mountInto(stores)
        var now = 0L
        var reads = 0
        val local = LocalAuthority(
            stores = {
                reads++
                stores
            },
            mountInfo = { moduleMountInfo(stores) },
            clock = { now },
        )

        val first = local.roots()
        now += 59_000_000_000L
        val second = local.roots()
        now += 2_000_000_000L
        local.roots()

        assertSame(first, second)
        assertEquals(2, reads)
    }

    @Test
    fun givenNoRootsAtFirst_whenTheStoresAreMountedLater_thenTheRootsAreFound() {
        val local = LocalAuthority({ stores }, { moduleMountInfo(stores) })
        assertNull(local.roots())

        authority.mountInto(stores)

        assertNotNull(local.roots())
    }

    @Test
    fun givenTheNamesOfCaRs_whenCompared_thenACertificateIsNamedByItsSubjectOrIssuer() {
        assertTrue(LocalAuthority.named(authority.system.certificate))
        assertTrue(LocalAuthority.named(authority.cross))
        assertTrue(LocalAuthority.named(authority.presented("example.com").first()), "issued by the user root")
        assertFalse(LocalAuthority.named(PublicAuthority().presented("example.com").first()))
        assertFalse(LocalAuthority.named(selfSigned("ADBlocker system CA", organization = "Other").certificate))
    }
}
