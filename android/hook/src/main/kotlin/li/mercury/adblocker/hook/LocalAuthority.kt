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

import java.io.File
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import javax.security.auth.x500.X500Principal

/** The two certificate stores of this process: the system's and the user's. */
internal class Stores(val system: File, val user: File)

/**
 * ADBlocker's roots for this device, as the certificate stores of this
 * process hold them.
 *
 * The module adds its roots to copies of the stores that it mounts into
 * the processes of the apps it decrypts, and nowhere else
 * (`abd/src/trust.rs`). Each is the file the store names after the
 * certificate's subject, `.0`, which the module writes over any other
 * certificate of that name in its copy. A root is read only from a store
 * that the mount table ([mountInfo]) shows to be the module's copy
 * ([ModuleMounts]); where the system store is not, the module has not
 * mounted the stores into this process, and there is nothing to recognize.
 *
 * A root is recognized by subject and public key, the way Android finds a
 * trust anchor: the key is made on the device, so no other certificate
 * with the same name has it.
 */
internal class LocalAuthority(
    private val stores: () -> Stores,
    private val mountInfo: () -> String,
    private val clock: () -> Long = System::nanoTime,
) {
    private class Loaded(val roots: Roots, val at: Long)

    @Volatile
    private var loaded: Loaded? = null

    /**
     * The roots, or null where this process does not have them. Read again
     * after [REUSE_NANOS], so that a root made anew while the app runs is
     * known; an absence is not remembered, as the stores can be mounted
     * after the app started.
     */
    fun roots(): Roots? {
        val now = clock()
        loaded?.takeIf { now - it.at < REUSE_NANOS }?.let { return it.roots }
        val roots = read(stores()) ?: return null
        loaded = Loaded(roots, now)
        return roots
    }

    private fun read(stores: Stores): Roots? {
        val mounts = mountInfo()
        if (!ModuleMounts.isModuleCopy(mounts, stores.system.path, ModuleMounts.SYSTEM_COPY)) return null
        val system = rootAt(File(stores.system, SYSTEM_ROOT_FILE), SYSTEM_NAME) ?: return null
        val user = if (ModuleMounts.isModuleCopy(mounts, stores.user.path, ModuleMounts.USER_COPY)) {
            rootAt(File(stores.user, USER_ROOT_FILE), USER_NAME)
        } else {
            null
        }
        return Roots(system, user)
    }

    private fun rootAt(file: File, name: X500Principal): X509Certificate? {
        if (!file.isFile || file.length() > MAX_CERTIFICATE_BYTES) return null
        val certificate = file.inputStream().use {
            CertificateFactory.getInstance("X.509").generateCertificate(it)
        } as? X509Certificate
        return certificate?.takeIf { it.subjectX500Principal == name }
    }

    companion object {
        // The names `trust::store_name` gives the roots; a test there keeps
        // them so.
        const val SYSTEM_ROOT_FILE = "494499dc.0"
        const val USER_ROOT_FILE = "5554896d.0"

        // As `abd-engine/src/ca.rs` names the roots. X500Principal compares
        // names in canonical form, whatever string type encodes them.
        val SYSTEM_NAME = X500Principal("CN=ADBlocker system CA, O=ADBlocker")
        val USER_NAME = X500Principal("CN=ADBlocker user CA, O=ADBlocker")

        private const val REUSE_NANOS = 60_000_000_000L
        private const val MAX_CERTIFICATE_BYTES = 64 * 1024L

        /** Whether [certificate] is one of the roots or was issued by one, by name alone. */
        fun named(certificate: X509Certificate): Boolean = certificate.subjectX500Principal.isRootName() ||
            certificate.issuerX500Principal.isRootName()

        private fun X500Principal.isRootName() = this == SYSTEM_NAME || this == USER_NAME
    }
}

/**
 * The roots found in the module's copies of this process's stores. The
 * user root, in the copy of the user's store, is counted only beside the
 * system root.
 */
internal class Roots(private val system: X509Certificate, private val user: X509Certificate?) {
    fun isAnchor(certificate: X509Certificate): Boolean = sameRoot(certificate, system) ||
        (user != null && sameRoot(certificate, user))

    private fun sameRoot(certificate: X509Certificate, root: X509Certificate): Boolean = certificate.subjectX500Principal == root.subjectX500Principal &&
        certificate.publicKey.encoded.contentEquals(root.publicKey.encoded)
}
