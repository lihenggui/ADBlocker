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

import android.util.Base64
import li.mercury.adblocker.core.data.AbdException
import li.mercury.adblocker.core.data.transport.AbdTransport
import li.mercury.adblocker.core.data.transport.RootTransport
import li.mercury.adblocker.core.data.transport.TransportSource
import li.mercury.adblocker.core.engine.UserCa
import javax.inject.Inject

internal class DefaultHttpsRepository(
    private val transports: TransportSource,
    private val rememberCertificate: (ByteArray) -> Unit,
    private val isCertificateInstalled: () -> Boolean,
) : HttpsRepository {
    @Inject
    constructor(transports: TransportSource, userCa: UserCa) : this(transports, userCa::remember, userCa::installed)

    override suspend fun exportCertificate(): ByteArray = certificate(transports.current()).also(rememberCertificate)

    override suspend fun exportWorkCertificate(): ByteArray {
        val root = (transports.current() as? RootTransport) ?: throw AbdException.CommandFailed("Work profile certificate export requires the module")
        return certificate(root)
    }

    private suspend fun certificate(transport: AbdTransport): ByteArray {
        // PEM over the transport, which carries text; DER for the installer.
        val pem = transport.run(listOf("ca", "export"), "ca export")
        val body = pem.substringAfter("-----BEGIN CERTIFICATE-----", "").substringBefore("-----END CERTIFICATE-----", "")
        if (body.isBlank()) throw AbdException.BadOutput("ca export: no certificate")
        return Base64.decode(body.filterNot { it.isWhitespace() }, Base64.DEFAULT)
    }

    override fun certificateInstalled(): Boolean = isCertificateInstalled()

    override suspend fun reload() {
        transports.current().run(listOf("reload"), "reload")
    }
}
