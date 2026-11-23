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
import android.os.Bundle;
import java.io.IOException;
import java.io.InputStream;
import java.security.GeneralSecurityException;
import java.security.cert.CertificateFactory;

/** Provisions only the public certificate bundled into this check APK by its builder. */
public final class TrustCheckCa extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        DevicePolicyManager policy = getSystemService(DevicePolicyManager.class);
        if (policy.isProfileOwnerApp(getPackageName())) {
            ComponentName admin = new ComponentName(this, CheckAdmin.class);
            try (InputStream certificate = getAssets().open("test-ca.pem")) {
                byte[] encoded = CertificateFactory.getInstance("X.509")
                        .generateCertificate(certificate).getEncoded();
                if (getIntent().getBooleanExtra("remove", false)) {
                    policy.uninstallCaCert(admin, encoded);
                } else if (!policy.installCaCert(admin, encoded)) {
                    throw new IllegalStateException("Profile CA installation failed");
                }
            } catch (IOException | GeneralSecurityException e) {
                throw new IllegalStateException("Cannot read the test CA", e);
            }
        }
        finish();
    }
}
