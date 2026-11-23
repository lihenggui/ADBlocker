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

package li.mercury.adblocker.check.http3;

import android.app.Activity;
import android.os.Bundle;
import java.io.File;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.List;
import java.util.concurrent.TimeUnit;

/** Only shell or instrumentation can launch this synthetic client. */
public final class Probe extends Activity {
    @Override
    public void onCreate(Bundle state) {
        super.onCreate(state);
        String peer = getIntent().getBooleanExtra("ipv6", false)
                ? "[2001:db8:ad::2]:443" : "198.18.0.2:443";
        String probe = getIntent().getStringExtra("probe");
        new Thread(() -> {
            File cert = new File(getFilesDir(), "cert.der");
            File result = new File(getFilesDir(), "result");
            File pending = new File(getFilesDir(), "result.new");
            try {
                try (InputStream in = getAssets().open("cert.der")) {
                    Files.copy(in, cert.toPath(), StandardCopyOption.REPLACE_EXISTING);
                }
                File client = new File(getApplicationInfo().nativeLibraryDir, "libh3check.so");
                List<String> args = "dual-fetch".equals(probe)
                        ? List.of(client.getPath(), probe, peer)
                        : List.of(client.getPath(), "fetch", peer, cert.getPath());
                ProcessBuilder command = new ProcessBuilder(args);
                Process process = command
                        .redirectErrorStream(true).redirectOutput(pending).start();
                if (!process.waitFor(7, TimeUnit.SECONDS)) {
                    process.destroyForcibly();
                    process.waitFor(1, TimeUnit.SECONDS);
                    Files.write(pending.toPath(), "handshake timeout".getBytes(StandardCharsets.UTF_8));
                }
                Files.move(pending.toPath(), result.toPath(), StandardCopyOption.REPLACE_EXISTING);
            } catch (Exception e) {
                try {
                    Files.write(result.toPath(), e.toString().getBytes(StandardCharsets.UTF_8));
                } catch (java.io.IOException ignored) { }
            } finally {
                runOnUiThread(this::finish);
            }
        }).start();
    }
}
