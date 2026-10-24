# R8/ProGuard rules for apps using marknotes-sync. R8 applies them automatically from the jar.
# JNA looks up the generated bindings and its own classes reflectively, by name and field order.
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class io.github.maxdac.marknotes.sync.** { *; }
