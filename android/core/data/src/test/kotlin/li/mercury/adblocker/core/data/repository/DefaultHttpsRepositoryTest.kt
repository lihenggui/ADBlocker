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

package li.mercury.adblocker.core.data.repository

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.root.CommandResult
import li.mercury.adblocker.core.data.root.ModulePaths.ABD
import li.mercury.adblocker.core.data.root.RootShell
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.testing.unexpected
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class DefaultHttpsRepositoryTest {
    private val pem = "-----BEGIN CERTIFICATE-----\n AQID\n BA== \n-----END CERTIFICATE-----\n"
    private val der = byteArrayOf(1, 2, 3, 4)
    private val remembered = mutableListOf<ByteArray>()
    private val commands = mutableListOf<List<String>>()
    private var response = pem
    private var installed = false
    private val vpn = object : AbdTransport by unexpected() {
        override suspend fun run(args: List<String>, label: String, input: String?): String {
            commands += args
            return response
        }
    }
    private val root = RootTransport(
        object : RootShell(Dispatchers.Unconfined) {
            override suspend fun run(command: String, label: String, timeoutSeconds: Int): CommandResult {
                assertEquals("$ABD 'ca' 'export'", command)
                assertEquals("abd ca export", label)
                return CommandResult(true, 0, pem, "")
            }
        },
    )
    private var active: AbdTransport = vpn
    private val repository = DefaultHttpsRepository(
        object : TransportSource {
            override suspend fun current() = active
        },
        { remembered += it },
        { installed },
    )

    @Test
    fun givenPersonalPemOutput_whenExported_thenTheDerIsReturnedAndRemembered() = runTest {
        assertContentEquals(der, repository.exportCertificate())
        assertEquals(listOf(listOf("ca", "export")), commands)
        assertContentEquals(der, remembered.single())
        assertFalse(repository.certificateInstalled())
        installed = true
        assertTrue(repository.certificateInstalled())
    }

    @Test
    fun givenWorkExport_whenTheMethodSwitchesToRoot_thenItUsesRootAndLeavesPersonalTrustAlone() = runTest {
        assertFailsWith<AbdException.CommandFailed> { repository.exportWorkCertificate() }
        assertTrue(commands.isEmpty())

        active = root
        assertContentEquals(der, repository.exportWorkCertificate())
        assertTrue(remembered.isEmpty())
    }

    @Test
    fun givenMissingCertificateOutput_whenExported_thenThePreviouslyRememberedCertificateIsKept() = runTest {
        repository.exportCertificate()
        response = "no certificate"
        assertFailsWith<AbdException.BadOutput> { repository.exportCertificate() }
        assertContentEquals(der, remembered.single())
    }

    @Test
    fun givenTheActiveTransport_whenReloaded_thenItReceivesTheReloadCommand() = runTest {
        repository.reload()
        assertEquals(listOf(listOf("reload")), commands)
    }
}
