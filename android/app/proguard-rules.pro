# Minification is currently off. If it is ever enabled, JNA and the UniFFI bindings are loaded
# reflectively and must be kept.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class uniffi.** { *; }
