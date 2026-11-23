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

package li.mercury.adblocker.test;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.DnsResolver;
import android.net.InetAddresses;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.CancellationSignal;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.StringJoiner;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;

/** Runs as the test APK's own UID, outside the VPN provider and its instrumentation. */
public final class VpnProbe extends BroadcastReceiver {
    private static final String HTTPS_HOST = "example.com";

    @Override
    public void onReceive(Context context, Intent intent) {
        PendingResult result = goAsync();
        new Thread(() -> {
            ConnectivityManager connectivity = context.getSystemService(ConnectivityManager.class);
            NetworkCapabilities net = connectivity.getNetworkCapabilities(connectivity.getActiveNetwork());
            boolean vpn = net != null && net.hasTransport(NetworkCapabilities.TRANSPORT_VPN);
            try {
                if (intent.hasExtra("waitForVpn")) {
                    vpn = intent.getBooleanExtra("waitForVpn", false);
                    if (!awaitDefaultTransport(connectivity, vpn)) {
                        result.setResultData("network transition timed out");
                        return;
                    }
                }
                String dnsHost = intent.getStringExtra("dnsHost");
                if (dnsHost != null) {
                    LinkProperties properties = connectivity.getLinkProperties(connectivity.getActiveNetwork());
                    boolean ipv6 = properties != null && properties.getRoutes().stream()
                        .anyMatch(route -> route.isDefaultRoute() && route.getDestination().getAddress() instanceof Inet6Address);
                    result.setResultData(vpn + " " + ipv6 + "\n"
                        + resolve(dnsHost, DnsResolver.TYPE_A) + "\n"
                        + (ipv6 ? resolve(dnsHost, DnsResolver.TYPE_AAAA) : "no IPv6 route"));
                    return;
                }
                result.setResultData(vpn + " " + fetchHttps(intent.getStringExtra("httpsAddress")));
            } catch (IOException e) {
                result.setResultData(vpn + " failed");
            } finally {
                result.finish();
            }
        }, "vpn-device-probe").start();
    }

    private static String fetchHttps(String address) throws IOException {
        // A cached DNS sinkhole can outlive a rule change. Keep testing the
        // same server and verify its hostname over TLS independently of DNS.
        try (Socket tcp = new Socket()) {
            tcp.connect(new InetSocketAddress(InetAddresses.parseNumericAddress(address), 443), 4000);
            SSLSocketFactory factory = (SSLSocketFactory) SSLSocketFactory.getDefault();
            try (SSLSocket tls = (SSLSocket) factory.createSocket(tcp, HTTPS_HOST, 443, true)) {
                SSLParameters parameters = tls.getSSLParameters();
                parameters.setEndpointIdentificationAlgorithm("HTTPS");
                tls.setSSLParameters(parameters);
                tls.setSoTimeout(4000);
                String request = "GET / HTTP/1.1\r\nHost: " + HTTPS_HOST + "\r\nConnection: close\r\n\r\n";
                tls.getOutputStream().write(request.getBytes(StandardCharsets.US_ASCII));
                BufferedReader response = new BufferedReader(new InputStreamReader(tls.getInputStream(), StandardCharsets.US_ASCII));
                String line = response.readLine();
                if (line == null || !line.startsWith("HTTP/1.")) throw new IOException("missing HTTP status");
                String[] status = line.split(" ", 3);
                if (status.length < 2) throw new IOException("missing HTTP status");
                return status[1];
            }
        }
    }

    private static String resolve(String host, int type) {
        CancellationSignal cancellation = new CancellationSignal();
        CompletableFuture<String> reply = new CompletableFuture<>();
        // Caches must not reuse an answer across the membership changes.
        int flags = DnsResolver.FLAG_NO_CACHE_LOOKUP | DnsResolver.FLAG_NO_CACHE_STORE;
        DnsResolver.getInstance().query(null, host, type, flags, Runnable::run, cancellation,
            new DnsResolver.Callback<List<InetAddress>>() {
                @Override
                public void onAnswer(List<InetAddress> addresses, int rcode) {
                    StringJoiner values = new StringJoiner(",");
                    for (InetAddress address : addresses) {
                        values.add(address instanceof Inet6Address && address.isAnyLocalAddress()
                            ? "::" : address.getHostAddress());
                    }
                    reply.complete(rcode == 0 ? values.toString() : "DNS rcode " + rcode);
                }

                @Override
                public void onError(DnsResolver.DnsException error) {
                    reply.complete("resolver error " + error.code + ": " + error.getCause());
                }
            });
        try {
            return reply.get(4, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return "resolver interrupted";
        } catch (ExecutionException | TimeoutException e) {
            return "resolver " + e.getClass().getSimpleName();
        } finally {
            cancellation.cancel();
        }
    }

    private static boolean awaitDefaultTransport(ConnectivityManager connectivity, boolean vpn) {
        CountDownLatch changed = new CountDownLatch(1);
        ConnectivityManager.NetworkCallback callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                if (vpn == capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) changed.countDown();
            }
        };
        connectivity.registerDefaultNetworkCallback(callback);
        try {
            return changed.await(5, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return false;
        } finally {
            connectivity.unregisterNetworkCallback(callback);
        }
    }
}
