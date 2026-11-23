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

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.VpnService;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

/** A disposable work VPN. It answers probes from its TUN. */
public final class CheckVpn extends VpnService {
    private static final String CHANNEL = "vpn-check";
    private volatile ParcelFileDescriptor tun;

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(
                CHANNEL, "ADBlocker VPN check", NotificationManager.IMPORTANCE_LOW));
        Notification notification = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_menu_info_details)
                .setContentTitle("ADBlocker managed VPN check").build();
        int type = Build.VERSION.SDK_INT >= 34 ? ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED
                : ServiceInfo.FOREGROUND_SERVICE_TYPE_NONE;
        startForeground(1, notification, type);
        boolean connected = getSharedPreferences("vpn-check", MODE_PRIVATE)
                .getBoolean("connected", false);
        if (!connected) {
            disconnect();
        } else if (tun == null) {
            tun = new Builder().setSession("ADBlocker VPN check")
                    .setMtu(1500).setBlocking(true)
                    .addAddress("10.81.0.2", 32).addRoute("0.0.0.0", 0).establish();
            if (tun == null) {
                throw new IllegalStateException("Android did not establish the always-on VPN");
            }
            ParcelFileDescriptor stream = tun;
            report(this, "vpn-state", "connected");
            new Thread(() -> serve(stream), "check-tun").start();
        }
        return START_STICKY;
    }

    private void serve(ParcelFileDescriptor stream) {
        try {
            FileInputStream from = new FileInputStream(stream.getFileDescriptor());
            FileOutputStream to = new FileOutputStream(stream.getFileDescriptor());
            new TunCheckOrigin(to, request -> report(this, "vpn-request", request)).serve(from);
            if (tun == stream) {
                report(this, "vpn-state", "unexpected TUN EOF");
            }
        } catch (IOException e) {
            if (tun == stream) {
                report(this, "vpn-state", "TUN I/O failed: " + e.getClass().getSimpleName());
            }
        }
    }

    private void disconnect() {
        ParcelFileDescriptor stream = tun;
        tun = null;
        if (stream != null) {
            try {
                stream.close();
            } catch (IOException ignored) {
                // Android has already detached this descriptor.
            }
        }
        report(this, "vpn-state", "disconnected");
    }

    @Override
    public void onDestroy() {
        disconnect();
        super.onDestroy();
    }

    static void report(Context context, String filename, String text) {
        File file = new File(context.getFilesDir(), filename);
        File pending = new File(file.getPath() + ".new");
        try (FileOutputStream out = new FileOutputStream(pending)) {
            out.write((text + "\n").getBytes(StandardCharsets.UTF_8));
        } catch (IOException e) {
            throw new IllegalStateException("Cannot write VPN check evidence", e);
        }
        if (!pending.renameTo(file)) {
            throw new IllegalStateException("Cannot publish VPN check evidence");
        }
    }
}
