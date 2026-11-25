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

package li.mercury.adblocker.core.model

/** How the chosen apps' connections reach the filtering engine. */
enum class FilterMethod {
    /** The root module's daemon takes them with packet filter rules. */
    Root,

    /** A VPN in the app takes them; no root needed. */
    Vpn,
}

/**
 * What can be done with a [FilterMethod]. Screens ask these rather than the
 * method, so that what differs between the two is said in one place.
 */
data class Capabilities(val method: FilterMethod) {
    /** The module is installed, updated and started from the app. */
    val managesModule: Boolean get() = method == FilterMethod.Root

    /** The module turns itself off after startups that did not finish. */
    val hasBootGuard: Boolean get() = method == FilterMethod.Root

    /** Takes the system's one VPN: no other VPN can run meanwhile. */
    val usesVpn: Boolean get() = method == FilterMethod.Vpn

    /** Decryption needs the authority in the user store, installed by the user. */
    val needsUserCa: Boolean get() = method == FilterMethod.Vpn
}

enum class VpnTransferPart { Apps, Lists, Rules, TrustedSites, UrlFiltering }
