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

package li.mercury.adblocker.core.engine

import android.content.Context
import androidx.core.content.edit
import dagger.hilt.android.qualifiers.ApplicationContext
import timber.log.Timber
import java.security.KeyStore
import java.security.MessageDigest
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The system's certificate stores, as the key store shows them to an app:
 * the roots servers are verified against, and whether the user installed
 * ADBlocker's authority.
 */
@Singleton
class UserCa @Inject internal constructor(
    @ApplicationContext context: Context,
) {
    // Written when the HTTPS setup exports the certificate: the callback that
    // asks cannot ask the engine back for it.
    private val prefs = context.getSharedPreferences("user_ca", Context.MODE_PRIVATE)

    /**
     * The system roots the user has not turned off. Never the user store's:
     * ADBlocker's own authority is among them.
     */
    fun systemRoots(): Array<ByteArray> = try {
        val store = KeyStore.getInstance(STORE).apply { load(null) }
        store.aliases().toList().filter { it.startsWith(SYSTEM) }.mapNotNull { store.getCertificate(it)?.encoded }.toTypedArray()
    } catch (e: Exception) {
        Timber.w("system roots: %s", e.javaClass.simpleName)
        emptyArray()
    }

    /** Remembers the authority's certificate, as exported for the user to install. */
    fun remember(der: ByteArray) {
        prefs.edit { putString(FINGERPRINT, sha256(der)) }
    }

    /** Whether the remembered authority is in the user store. */
    fun installed(): Boolean {
        val fingerprint = prefs.getString(FINGERPRINT, null) ?: return false
        return try {
            val store = KeyStore.getInstance(STORE).apply { load(null) }
            store.aliases().toList().filter { it.startsWith(USER) }.any { alias ->
                store.getCertificate(alias)?.encoded?.let(::sha256) == fingerprint
            }
        } catch (e: Exception) {
            Timber.w("user store: %s", e.javaClass.simpleName)
            false
        }
    }

    private fun sha256(der: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(der).joinToString("") { "%02x".format(it) }

    private companion object {
        const val STORE = "AndroidCAStore"
        const val SYSTEM = "system:"
        const val USER = "user:"
        const val FINGERPRINT = "fingerprint"
    }
}
