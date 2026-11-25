-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile

# R8 gives the last parameter of emitAbort, and of the bridge that calls it,
# sometimes Continuation and sometimes ContinuationImpl, from run to run with
# the same input: a release built again (as F-Droid does) would differ from
# it. Kept as declared, every build types it the same.
-keepclassmembers,allowobfuscation,allowshrinking class kotlinx.coroutines.flow.FlowKt__LimitKt {
    *** *emitAbort$FlowKt__LimitKt(...);
}
