# The Xposed API is compileOnly: the framework provides it when it loads the
# hook, so its classes are missing at build time.
-dontwarn io.github.libxposed.annotation.**
-dontwarn io.github.libxposed.api.**

# Keep the hook package as written, members included.
#
# LSPosed loads the entry class by the name listed in
# META-INF/xposed/java_init.list, a Java resource of this library module that
# R8 does not rewrite when the app is built.
#
# The members must be kept too. The hookers implement an interface of the
# Xposed API (XposedInterface.Hooker); with it missing, R8 cannot see that
# intercept() implements a framework method, and would strip or rename it.
-keep class li.mercury.adblocker.hook.** {
    *;
}
