#!/bin/sh
#
# Copyright (C) 2026 Mercury Li
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 3 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program. If not, see <https://www.gnu.org/licenses/>.

set -eu
cd "$(dirname "$0")/.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT

# Exercise the native entry points on a desktop JVM without Android services.
target_dir=${CARGO_TARGET_DIR:-target}
cargo build --locked -p abd-vpn --lib --target-dir "$target_dir"
library="$(cd "$target_dir" && pwd)/debug/libabd_vpn.so"
mkdir "$fixture/classes"
touch "$fixture/descriptor"
cat >"$fixture/Engine.java" <<'JAVA'
package li.mercury.adblocker.core.engine;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.cert.CertificateFactory;

public class Engine {
    private native long open(String dir, byte[][] roots, String[] localeTags, String timeZoneId, Callbacks callbacks);
    private native Result command(long handle, String[] args, byte[] input);
    private native long stream(long handle, String[] args, Sink sink);
    private native int start(long handle, int tunFd, boolean ipv6, String[] resolvers);
    private native void close(long handle);

    public static void main(String[] arguments) throws Exception {
        System.load(arguments[0]);
        byte[] trustAnchor;
        try (var input = Files.newInputStream(Path.of(arguments[1]))) {
            trustAnchor = CertificateFactory.getInstance("X.509").generateCertificate(input).getEncoded();
        }
        Path work = Path.of(arguments[2]);
        var engine = new Engine();
        var callbacks = new Callbacks();
        String dir = work.resolve("state").toString();

        String[] localeTags = {"en-US", "en-KR"};
        String timeZoneId = "Asia/Shanghai";
        check(engine.open(dir, new byte[0][], localeTags, timeZoneId, callbacks) == 0, "no upstream roots");
        var unopened = engine.command(0, new String[]{"status", "--json"}, null);
        check(unopened.exit() == 1 && unopened.stdout().length == 0, "closed command status");
        check(new String(unopened.stderr(), StandardCharsets.UTF_8).equals("the engine is not open\n"), "closed command error");
        var sink = new Sink();
        check(engine.stream(0, new String[]{"watch", "--json"}, sink) == 0, "closed stream id");
        check(sink.calls == 1 && sink.exit == 1, "closed stream outcome");
        check(engine.start(0, -1, false, new String[0]) == 2, "negative descriptor");

        engine.rejectDescriptor(0, 9, new String[0]);

        long handle = engine.open(dir, new byte[][]{trustAnchor}, localeTags, timeZoneId, callbacks);
        check(handle != 0, "engine with usable upstream roots");
        try {
            engine.rejectDescriptor(handle, 8, null);
            engine.rejectDescriptor(handle, 7, new String[0]);
            String lists = engine.run(handle, "lists", "--json");
            check(lists.contains("easylistchina.txt"), "the time zone enables the Chinese defaults");
            check(lists.contains("koreanlist.txt"), "the secondary locale enables the Korean default");
            engine.run(handle, "app", "add", "com.example.news");
            check(callbacks.uidCalls > 0, "UID callback exercised");
            check(engine.run(handle, "status", "--json").contains("\"url_filtering\":true"), "URL filtering on by default");
            String apps = engine.run(handle, "app", "list", "--json");
            check(apps.contains("ca_not_installed") && callbacks.caCalls > 0, "CA callback failure stays encrypted");
            String state = Files.readString(Path.of(dir, "state"));
            check(state.contains("\"filtering_apps\":0"), "UID callback failure leaves no filtered apps");

            engine.run(handle, "scope", "all");
            check(callbacks.inventoryCalls > 0, "inventory callback exercised");
            state = Files.readString(Path.of(dir, "state"));
            check(state.contains("\"filtering_apps\":0"), "inventory callback failure leaves no filtered apps");
        } finally {
            engine.close(handle);
        }
        System.out.println("Host JNI failure checks passed");
    }

    // The shell gives each case a separate open regular file. The native
    // start call owns it even when it cannot reach or start the session.
    private void rejectDescriptor(long handle, int descriptor, String[] resolvers) {
        Path entry = Path.of("/proc/self/fd/" + descriptor);
        check(Files.isSymbolicLink(entry), "test descriptor is open");
        check(start(handle, descriptor, false, resolvers) == 1, "failed startup status");
        check(!Files.isSymbolicLink(entry), "failed startup closed the descriptor");
    }

    private String run(long handle, String... args) {
        Result result = command(handle, args, null);
        check(result.exit() == 0, "command exit for " + String.join(" ", args));
        return new String(result.stdout(), StandardCharsets.UTF_8);
    }

    static void check(boolean condition, String description) {
        if (!condition) throw new AssertionError(description);
    }
}

record Result(int exit, byte[] stdout, byte[] stderr) {}

class Sink {
    int calls;
    int exit;
    public void line(String text) { throw new AssertionError("unexpected line"); }
    public void end(int code) {
        calls++;
        exit = code;
    }
}

class Callbacks {
    int uidCalls;
    int caCalls;
    int inventoryCalls;

    public int uidOf(String name) {
        uidCalls++;
        throw new IllegalStateException("test UID failure");
    }

    public boolean userCaInstalled() {
        caCalls++;
        throw new IllegalStateException("test CA failure");
    }

    public String personalAppIdentities(String excluded) {
        inventoryCalls++;
        throw new IllegalStateException("test inventory failure");
    }
}
JAVA

javac -d "$fixture/classes" "$fixture/Engine.java"
java --enable-native-access=ALL-UNNAMED -cp "$fixture/classes" \
    li.mercury.adblocker.core.engine.Engine "$library" abd/testdata/isrg-root-x1.pem "$fixture" \
    7<"$fixture/descriptor" 8<"$fixture/descriptor" 9<"$fixture/descriptor"
