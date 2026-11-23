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

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.function.Consumer;

/** A bounded HTTP and UDP fixture at the test-only address 192.0.2.17. */
final class TunCheckOrigin {
    private static final int ADDRESS = 0xc0000211;
    private static final int FIN = 1;
    private static final int SYN = 2;
    private static final int RST = 4;
    private static final int PSH = 8;
    private static final int ACK = 16;
    private static final int BUFFER = 4096;
    private static final byte[] EMPTY = new byte[0];
    private static final byte[] UDP_PROBE = "abd-test".getBytes(StandardCharsets.US_ASCII);
    private static final byte[] UDP_REPLY = "managed-tun-udp".getBytes(StandardCharsets.US_ASCII);
    private static final byte[] RESPONSE = ("HTTP/1.1 200 OK\r\nContent-Length: 17\r\n"
            + "Connection: close\r\n\r\nmanaged-tun-proof").getBytes(StandardCharsets.US_ASCII);

    private final OutputStream packets;
    private final Consumer<String> observed;
    private final Map<Integer, Flow> flows = new LinkedHashMap<>();

    TunCheckOrigin(OutputStream packets, Consumer<String> observed) {
        this.packets = packets;
        this.observed = observed;
    }

    void serve(InputStream tun) throws IOException {
        byte[] bytes = new byte[BUFFER];
        for (int length; (length = tun.read(bytes)) > 0;) {
            receive(ByteBuffer.wrap(bytes, 0, length));
        }
    }

    private void receive(ByteBuffer frame) throws IOException {
        if (frame.limit() < 20 || (frame.get(0) & 0xf0) != 0x40
                || frame.getInt(16) != ADDRESS || (frame.getShort(6) & 0x3fff) != 0) {
            return;
        }
        int ip = (frame.get(0) & 15) * 4;
        int end = frame.getShort(2) & 0xffff;
        if (ip < 20 || end < ip || end > frame.limit()) {
            return;
        }
        if (frame.get(9) == 17) {
            receiveUdp(frame, ip, end);
            return;
        }
        if (frame.get(9) != 6 || end < ip + 20 || frame.getShort(ip + 2) != 80) {
            return;
        }
        int port = frame.getShort(ip) & 0xffff;
        int sequence = frame.getInt(ip + 4);
        int offset = (frame.get(ip + 12) & 0xf0) >> 2;
        int flags = frame.get(ip + 13) & 0xff;
        if (offset < 20 || ip + offset > end) {
            return;
        }
        if ((flags & RST) != 0) {
            flows.remove(port);
            return;
        }
        if ((flags & SYN) != 0) {
            if (flows.size() >= 8) {
                flows.remove(flows.keySet().iterator().next());
            }
            Flow flow = new Flow(frame.getInt(12), sequence + 1, 0x43210000 + port);
            flows.put(port, flow);
            send(port, flow, SYN | ACK, EMPTY);
            return;
        }
        Flow flow = flows.get(port);
        if (flow == null || flow.address != frame.getInt(12)) {
            return;
        }
        int payload = end - ip - offset;
        if (sequence != flow.incoming) {
            send(port, flow, ACK, EMPTY);
            return;
        }
        if (payload != 0 && !flow.replied) {
            if (flow.header.size() + payload > BUFFER) {
                flows.remove(port);
                return;
            }
            flow.header.write(frame.array(), ip + offset, payload);
            flow.incoming += payload;
            String text = flow.header.toString(StandardCharsets.US_ASCII.name());
            if (text.contains("\r\n\r\n")) {
                observed.accept(text.startsWith("GET /abd-managed-allowed ") ? "allowed"
                        : text.startsWith("GET /abd-managed-blocked ") ? "blocked" : "unexpected");
                flow.replied = true;
                send(port, flow, PSH | FIN | ACK, RESPONSE);
                return;
            }
            send(port, flow, ACK, EMPTY);
        }
        if ((flags & FIN) != 0) {
            flow.incoming++;
            send(port, flow, flow.replied ? ACK : FIN | ACK, EMPTY);
            flows.remove(port);
        }
    }

    private void receiveUdp(ByteBuffer frame, int ip, int end) throws IOException {
        if (end - ip != 8 + UDP_PROBE.length || (frame.getShort(ip + 2) & 0xffff) != 443
                || (frame.getShort(ip + 4) & 0xffff) != end - ip) {
            return;
        }
        for (int i = 0; i < UDP_PROBE.length; i++) {
            if (frame.get(ip + 8 + i) != UDP_PROBE[i]) {
                return;
            }
        }
        ByteBuffer out = replyHeader(8 + UDP_REPLY.length, 17, frame.getInt(12));
        // An IPv4 UDP datagram may omit the UDP checksum.
        out.putShort((short) 443).putShort(frame.getShort(ip));
        out.putShort((short) (8 + UDP_REPLY.length)).putShort((short) 0).put(UDP_REPLY);
        packets.write(out.array());
    }

    private static ByteBuffer replyHeader(int payloadLength, int protocol, int remote) {
        ByteBuffer out = ByteBuffer.allocate(20 + payloadLength);
        out.put((byte) 0x45).put((byte) 0).putShort((short) out.capacity());
        out.putInt(0).put((byte) 64).put((byte) protocol).putShort((short) 0);
        out.putInt(ADDRESS).putInt(remote);
        out.putShort(10, checksum(sum(out.array(), 0, 20)));
        return out;
    }

    private void send(int port, Flow flow, int flags, byte[] payload) throws IOException {
        ByteBuffer out = replyHeader(20 + payload.length, 6, flow.address);
        out.putShort((short) 80).putShort((short) port);
        out.putInt(flow.outgoing).putInt(flow.incoming);
        out.put((byte) 0x50).put((byte) flags).putShort((short) BUFFER);
        out.putInt(0).put(payload);
        byte[] packet = out.array();
        int pseudoHeader = sum(packet, 12, 20) + 6 + packet.length - 20;
        out.putShort(36, checksum(pseudoHeader + sum(packet, 20, packet.length)));
        packets.write(packet);
        flow.outgoing += payload.length + (((flags & (SYN | FIN)) != 0) ? 1 : 0);
    }

    private static int sum(byte[] bytes, int first, int end) {
        int sum = 0;
        for (int i = first; i < end; i += 2) {
            sum += (bytes[i] & 0xff) << 8;
            if (i + 1 < end) {
                sum += bytes[i + 1] & 0xff;
            }
        }
        return sum;
    }

    private static short checksum(int sum) {
        while ((sum >>> 16) != 0) {
            sum = (sum & 0xffff) + (sum >>> 16);
        }
        return (short) ~sum;
    }

    private static final class Flow {
        final int address;
        final ByteArrayOutputStream header = new ByteArrayOutputStream();
        int incoming;
        int outgoing;
        boolean replied;

        Flow(int address, int incoming, int outgoing) {
            this.address = address;
            this.incoming = incoming;
            this.outgoing = outgoing;
        }
    }
}
