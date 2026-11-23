# The engine's native code finds these by name (docs/engine-jni.md).
-keep class li.mercury.adblocker.core.engine.Engine { native <methods>; }
-keep class li.mercury.adblocker.core.engine.Result { <init>(int, byte[], byte[]); }
-keep interface li.mercury.adblocker.core.engine.Callbacks { *; }
-keep class * implements li.mercury.adblocker.core.engine.Callbacks {
    boolean protect(int);
    int ownerUid(int, byte[], int, byte[], int);
    int uidOf(java.lang.String);
    boolean userCaInstalled();
    void stopped(int);
}
-keep interface li.mercury.adblocker.core.engine.LineSink { *; }
-keep class * implements li.mercury.adblocker.core.engine.LineSink {
    void line(java.lang.String);
    void end(int);
}
