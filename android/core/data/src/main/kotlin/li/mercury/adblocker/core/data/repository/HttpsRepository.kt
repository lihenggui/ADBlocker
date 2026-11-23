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

/**
 * HTTPS filtering without root: the user installs ADBlocker's certificate
 * in the system's user store (docs/engine-jni.md, Decryption).
 */
interface HttpsRepository {
    /** The certificate, DER-encoded, for the system's installer; remembered to recognise it once installed. */
    suspend fun exportCertificate(): ByteArray

    /** The module's public certificate, for provisioning in a work profile. */
    suspend fun exportWorkCertificate(): ByteArray = error("Work profile certificate export is unavailable")

    /** Whether the user store holds the certificate. */
    fun certificateInstalled(): Boolean

    /** Has the engine read the certificate's state and the chosen apps again. */
    suspend fun reload()
}
