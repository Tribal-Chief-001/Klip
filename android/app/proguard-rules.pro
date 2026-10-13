# R8 is on. JNA and the UniFFI bindings are loaded reflectively and must be kept.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class uniffi.** { *; }
-dontwarn java.awt.**
# JNI entry points are looked up by name.
-keepclasseswithmembernames class * { native <methods>; }
-keep class com.klip.app.KlipRuntime { *; }
