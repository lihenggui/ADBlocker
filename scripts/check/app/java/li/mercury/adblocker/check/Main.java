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

package li.mercury.adblocker.check;

import android.app.Activity;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.os.Bundle;
import android.os.SystemClock;
import android.system.ErrnoException;
import android.system.OsConstants;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.HttpURLConnection;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.Socket;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.cert.X509Certificate;
import java.util.HashMap;
import java.util.Map;
import java.util.TreeSet;
import javax.net.ssl.HttpsURLConnection;

/**
 * Fetches the address in the intent's "url" extra with the platform's own HTTP client and trust,
 * and writes one line to the file "result" in the app's files: the status and who issued the
 * server's certificate ("-" where there is none, over plain HTTP), or the error.
 * A UDP probe instead returns the reply from the managed VPN fixture. A lookup writes the
 * addresses the system resolver gives for the host, or, asked of the network's DNS server
 * directly over UDP or TCP, the first IPv4 address of the answer and its lifetime.
 */
public class Main extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        final String address = getIntent().getStringExtra("url");
        final boolean readBody = getIntent().getBooleanExtra("body", false);
        final boolean datagram = getIntent().getBooleanExtra("udp", false);
        final boolean waitForProbe = getIntent().getBooleanExtra("waitForProbe", false);
        // Connects only when told to: the check first sees this process given
        // what the daemon gives a process as it starts.
        final boolean waitForGo = getIntent().getBooleanExtra("waitForGo", false);
        final boolean resolve = getIntent().getBooleanExtra("resolve", false);
        final String dns = getIntent().getStringExtra("dns");
        final File result = new File(getFilesDir(), "result");
        if (getIntent().getBooleanExtra("webview", false)) {
            openPage(address, result, getIntent().getBooleanExtra("expectCosmetic", false));
            return;
        }
        // Lets the daemon hold the app to a certificate refusal: one made
        // just after the process started counts for nothing.
        final int delayMs = getIntent().getIntExtra("delayMs", 0);
        new Thread(() -> {
            SystemClock.sleep(delayMs);
            if (waitForGo && !awaitProbe()) {
                write(result, "error not told to go");
                return;
            }
            if (address == null) {
                write(result, "error no url");
            } else if (resolve) {
                write(result, resolve(address));
            } else if (dns != null) {
                write(result, askServer(address, dns));
            } else if (waitForProbe) {
                write(result, awaitProbe() ? fetch(address, readBody) + " / " + fetchUdp(address)
                        : "error probe was not released");
            } else {
                write(result, datagram ? fetchUdp(address) : fetch(address, readBody));
            }
        }).start();
    }

    private void openPage(String address, File result, boolean cosmetic) {
        WebView view = new WebView(this);
        setContentView(view);
        view.getSettings().setJavaScriptEnabled(true);
        view.setWebViewClient(new WebViewClient() {
            @Override
            public WebResourceResponse shouldInterceptRequest(WebView web, WebResourceRequest request) {
                if (!request.isForMainFrame()) return null;
                // WebView may use ECH, which the module passes through.
                // Fetch through the selected app's platform TLS client,
                // then let Chromium render the response and enforce its CSP.
                return fetchPage(request.getUrl().toString(), result);
            }

            @Override
            public void onPageFinished(WebView web, String url) {
                readPage(web, result, cosmetic, SystemClock.elapsedRealtime() + 6000);
            }

            @Override
            public void onReceivedError(WebView web, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame()) write(result, "error WebView " + error.getErrorCode());
            }
        });
        // The fixture app starts cold, so let the daemon mount its stores
        // before WebView makes the first connection and reads its roots.
        view.postDelayed(() -> view.loadUrl(address), 1500);
    }

    private static WebResourceResponse fetchPage(String address, File result) {
        HttpsURLConnection connection = null;
        try {
            connection = (HttpsURLConnection) new URL(address).openConnection(Proxy.NO_PROXY);
            connection.setConnectTimeout(5000);
            connection.setReadTimeout(5000);
            connection.setInstanceFollowRedirects(false);
            connection.setRequestProperty("Sec-Fetch-Dest", "document");
            int status = connection.getResponseCode();
            X509Certificate leaf = (X509Certificate) connection.getServerCertificates()[0];
            if (status != 200 || !leaf.getIssuerX500Principal().getName().contains("ADBlocker")) {
                throw new IOException("response " + status + " without the expected filtering certificate");
            }
            Map<String, String> headers = new HashMap<>();
            connection.getHeaderFields().forEach((name, values) -> {
                if (name != null) headers.put(name, String.join(", ", values));
            });
            try (InputStream input = connection.getInputStream()) {
                byte[] body = input.readNBytes(512 * 1024 + 1);
                if (body.length > 512 * 1024) throw new IOException("fixture response exceeded its bound");
                return new WebResourceResponse(
                        "text/html", "UTF-8", 200, "OK", headers, new ByteArrayInputStream(body));
            }
        } catch (Exception error) {
            write(result, "error fixture HTTPS " + error.getClass().getSimpleName() + ": " + error.getMessage());
            return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0]));
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    private void readPage(WebView view, File result, boolean cosmetic, long deadline) {
        if (result.exists()) return;
        String probe = "[window.fixtureAtStart, window.fixtureForbidden === true,"
                + " getComputedStyle(document.getElementById('advert')).display === 'none',"
                + " getComputedStyle(document.getElementById('control')).display !== 'none']";
        view.evaluateJavascript(probe, state -> {
            String expected = cosmetic ? "[42,false,true,true]" : "[null,false,false,true]";
            if (result.exists()) return;
            if (expected.equals(state) || SystemClock.elapsedRealtime() >= deadline) {
                write(result, state);
            } else {
                view.postDelayed(() -> readPage(view, result, cosmetic, deadline), 100);
            }
        });
    }

    private boolean awaitProbe() {
        File trigger = new File(getFilesDir(), "probe-go");
        write(new File(getFilesDir(), "probe-ready"), "ready");
        long deadline = SystemClock.elapsedRealtime() + 20_000;
        while (SystemClock.elapsedRealtime() < deadline) {
            if (trigger.isFile()) {
                return true;
            }
            SystemClock.sleep(25);
        }
        return false;
    }

    private static String fetch(String address, boolean readBody) {
        try {
            if (lookupBlocked(new URL(address).getHost())) return "error lookup blocked";
            HttpURLConnection c = (HttpURLConnection) new URL(address).openConnection();
            c.setConnectTimeout(10000);
            c.setReadTimeout(10000);
            c.setInstanceFollowRedirects(false);
            int status = c.getResponseCode();
            String issuer = "-";
            if (c instanceof HttpsURLConnection) {
                X509Certificate leaf =
                        (X509Certificate) ((HttpsURLConnection) c).getServerCertificates()[0];
                issuer = leaf.getIssuerX500Principal().getName();
            }
            return status + " " + issuer + (readBody ? body(c, status) : "");
        } catch (Exception e) {
            return "error " + e.getClass().getSimpleName();
        }
    }

    private static String fetchUdp(String address) {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(new InetSocketAddress(new URL(address).getHost(), 443));
            socket.setSoTimeout(3000);
            byte[] probe = "abd-test".getBytes(StandardCharsets.US_ASCII);
            try {
                socket.send(new DatagramPacket(probe, probe.length));
            } catch (IOException e) {
                // The kernel can fail the send and still deliver port unreachable.
                if (!(e.getCause() instanceof ErrnoException)
                        || ((ErrnoException) e.getCause()).errno != OsConstants.EPERM) {
                    throw e;
                }
            }
            byte[] reply = new byte[32];
            DatagramPacket packet = new DatagramPacket(reply, reply.length);
            socket.receive(packet);
            return new String(reply, 0, packet.getLength(), StandardCharsets.US_ASCII);
        } catch (IOException e) {
            return "error " + e.getClass().getSimpleName();
        }
    }

    /**
     * Whether the host's lookup was answered as blocked: with nothing but the unspecified
     * address. The platform's client would connect there and fail for a reason of its own.
     */
    private static boolean lookupBlocked(String host) {
        try {
            for (InetAddress address : InetAddress.getAllByName(host)) {
                if (!address.isAnyLocalAddress()) return false;
            }
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    private static String resolve(String host) {
        try {
            TreeSet<String> found = new TreeSet<>();
            for (InetAddress address : InetAddress.getAllByName(host)) {
                found.add(address.getHostAddress());
            }
            return "resolved " + String.join(" ", found);
        } catch (IOException e) {
            return "error " + e.getClass().getSimpleName();
        }
    }

    /**
     * Asks the network's first DNS server for the IPv4 addresses of a host, over "udp" or "tcp";
     * or, for "kept", over one UDP socket: for first.host, then for second.host once released.
     */
    private String askServer(String host, String how) {
        try {
            ConnectivityManager connectivity = getSystemService(ConnectivityManager.class);
            LinkProperties link = connectivity.getLinkProperties(connectivity.getActiveNetwork());
            InetAddress server = null;
            for (InetAddress candidate : link.getDnsServers()) {
                if (server == null && candidate.getAddress().length == 4) server = candidate;
            }
            if (server == null) return "error no IPv4 DNS server";
            if (how.equals("kept")) return overKeptSocket(server, host);
            byte[] query = query(host);
            byte[] answer = how.equals("tcp") ? overTcp(server, query) : overUdp(server, query);
            return firstAddress(answer, query.length);
        } catch (IOException e) {
            return "error " + e.getClass().getSimpleName();
        }
    }

    private String overKeptSocket(InetAddress server, String host) throws IOException {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(new InetSocketAddress(server, 53));
            socket.setSoTimeout(5000);
            String first = askOn(socket, "first." + host);
            if (!awaitProbe()) return "error probe was not released";
            String second;
            try {
                second = askOn(socket, "second." + host);
            } catch (IOException e) {
                second = "error " + e.getClass().getSimpleName();
            }
            return first + " / " + second;
        }
    }

    private static String askOn(DatagramSocket socket, String host) throws IOException {
        byte[] query = query(host);
        socket.send(new DatagramPacket(query, query.length));
        byte[] reply = new byte[4096];
        DatagramPacket packet = new DatagramPacket(reply, reply.length);
        socket.receive(packet);
        return firstAddress(java.util.Arrays.copyOf(reply, packet.getLength()), query.length);
    }

    private static byte[] query(String host) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        DataOutputStream data = new DataOutputStream(out);
        data.writeShort((int) (SystemClock.elapsedRealtimeNanos() & 0xffff));
        data.writeShort(0x0100);
        data.writeShort(1);
        data.writeShort(0);
        data.writeInt(0);
        for (String label : host.split("\\.")) {
            data.writeByte(label.length());
            data.write(label.getBytes(StandardCharsets.US_ASCII));
        }
        data.writeByte(0);
        data.writeShort(1);
        data.writeShort(1);
        return out.toByteArray();
    }

    private static byte[] overUdp(InetAddress server, byte[] query) throws IOException {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(new InetSocketAddress(server, 53));
            socket.setSoTimeout(5000);
            socket.send(new DatagramPacket(query, query.length));
            byte[] reply = new byte[4096];
            DatagramPacket packet = new DatagramPacket(reply, reply.length);
            socket.receive(packet);
            return java.util.Arrays.copyOf(reply, packet.getLength());
        }
    }

    private static byte[] overTcp(InetAddress server, byte[] query) throws IOException {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(server, 53), 5000);
            socket.setSoTimeout(5000);
            DataOutputStream out = new DataOutputStream(socket.getOutputStream());
            out.writeShort(query.length);
            out.write(query);
            DataInputStream in = new DataInputStream(socket.getInputStream());
            byte[] reply = new byte[in.readUnsignedShort()];
            in.readFully(reply);
            return reply;
        }
    }

    /** "answer" and the first A record's address and lifetime, or "answer none". */
    private static String firstAddress(byte[] reply, int questionEnd) {
        int count = ((reply[6] & 0xff) << 8) | (reply[7] & 0xff);
        int at = questionEnd;
        for (int i = 0; i < count && at < reply.length; i++) {
            while ((reply[at] & 0xc0) == 0 && reply[at] != 0) at += (reply[at] & 0xff) + 1;
            at += (reply[at] & 0xc0) != 0 ? 2 : 1;
            int type = ((reply[at] & 0xff) << 8) | (reply[at + 1] & 0xff);
            long ttl = ((reply[at + 4] & 0xffL) << 24) | ((reply[at + 5] & 0xff) << 16)
                    | ((reply[at + 6] & 0xff) << 8) | (reply[at + 7] & 0xff);
            int length = ((reply[at + 8] & 0xff) << 8) | (reply[at + 9] & 0xff);
            at += 10;
            if (type == 1 && length == 4) {
                return "answer " + (reply[at] & 0xff) + "." + (reply[at + 1] & 0xff) + "."
                        + (reply[at + 2] & 0xff) + "." + (reply[at + 3] & 0xff) + " ttl " + ttl;
            }
            at += length;
        }
        return "answer none";
    }

    private static String body(HttpURLConnection c, int status) throws IOException {
        try (InputStream in = status >= 400 ? c.getErrorStream() : c.getInputStream()) {
            if (in == null) {
                return "";
            }
            byte[] bytes = new byte[32];
            int total = 0;
            while (total < bytes.length) {
                int size = in.read(bytes, total, bytes.length - total);
                if (size == -1) {
                    break;
                }
                total += size;
            }
            return total == 0 ? "" : " " + new String(bytes, 0, total, StandardCharsets.US_ASCII).trim();
        }
    }

    private static void write(File result, String line) {
        // Under another name first: the file is there only once it is whole.
        File tmp = new File(result.getPath() + ".new");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            out.write((line + "\n").getBytes(StandardCharsets.UTF_8));
        } catch (Exception e) {
            return;
        }
        tmp.renameTo(result);
    }
}
