# JNA finds native methods, Structure fields and callback methods by name through reflection.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-dontwarn java.awt.**

# The UniFFI bindings register native functions by their Java names and read Structure fields by name.
-keep class app.lagommail.core.** { *; }
