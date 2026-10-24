# R8 rules for the minified test APK only (the app under test uses just the rules shipped in the bindings jar).
-dontwarn com.google.errorprone.annotations.**
-keep class io.github.maxdac.marknotes.sync.** { *; }
