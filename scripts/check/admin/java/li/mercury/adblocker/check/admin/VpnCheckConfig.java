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

package li.mercury.adblocker.check.admin;

import android.app.Activity;
import android.app.admin.DevicePolicyManager;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;

public final class VpnCheckConfig extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        DevicePolicyManager policy = getSystemService(DevicePolicyManager.class);
        if (!policy.isProfileOwnerApp(getPackageName())) {
            throw new IllegalStateException("Only the disposable profile owner sets up this VPN");
        }
        ComponentName admin = new ComponentName(this, CheckAdmin.class);
        String action = getIntent().getStringExtra("action");
        boolean enabled = "connect".equals(action);
        getSharedPreferences("vpn-check", MODE_PRIVATE).edit().putBoolean("connected", enabled).commit();
        try {
            if (enabled) {
                policy.setAlwaysOnVpnPackage(admin, getPackageName(), true);
            } else if ("clear".equals(action)) {
                policy.setAlwaysOnVpnPackage(admin, null, false);
                stopService(new Intent(this, CheckVpn.class));
            } else if ("disconnect".equals(action)) {
                startService(new Intent(this, CheckVpn.class));
            } else {
                throw new IllegalArgumentException("Unknown VPN fixture action");
            }
        } catch (PackageManager.NameNotFoundException e) {
            throw new IllegalStateException("The test VPN package is missing", e);
        }
        CheckVpn.report(this, "vpn-policy", policy.getAlwaysOnVpnPackage(admin)
                + " " + policy.isAlwaysOnVpnLockdownEnabled(admin));
        finish();
    }
}
