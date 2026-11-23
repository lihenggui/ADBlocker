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

import java.lang.reflect.Method
import java.lang.reflect.Modifier

// Which methods are certificate checks to hook: by name and the types they
// take, which an app keeps for methods that implement Android's interfaces.

/** OkHttp's certificate pinner, under the name only an app that does not obfuscate OkHttp keeps. */
internal const val OKHTTP_PINNER = "okhttp3.CertificatePinner"

private val bootLoader = Any::class.java.classLoader
private val ownLoader = Accepted::class.java.classLoader

/** Whether [type] is the app's, not Android's or this module's. */
internal fun isAppClass(type: Class<*>): Boolean = type.classLoader !== bootLoader && type.classLoader !== ownLoader

/** A `checkServerTrusted` with code, in a form Android calls, that returns nothing or the chain. */
internal fun isServerCheck(method: Method): Boolean = method.name == "checkServerTrusted" &&
    !Modifier.isAbstract(method.modifiers) &&
    ServerCheck.isForm(method.parameterTypes) &&
    (method.returnType == Void.TYPE || method.returnType == List::class.java)

/** OkHttp's `check(String, List)`, or `check$okhttp(String, Function0)` from OkHttp 4. */
internal fun isOkHttpCheck(method: Method): Boolean {
    val parameters = method.parameterTypes
    if (parameters.size != 2 || parameters[0] != String::class.java) return false
    return when (method.name) {
        "check" -> parameters[1] == List::class.java
        "check\$okhttp" -> parameters[1].isInterface
        else -> false
    }
}
